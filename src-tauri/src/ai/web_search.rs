//! Bounded, zero-configuration Hosted MCP web search for AI sessions.
//!
//! The MCP service's text response is returned to the LLM verbatim — we do not
//! parse it into structured results, so rssh never breaks when Exa / Parallel
//! change their response shape (mirrors opencode's approach). The only local
//! processing is redacting the query before it leaves the device.

use std::fmt;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    sanitize::{self, RedactRule},
    tools::WebSearchInput,
};

const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";
const PARALLEL_MCP_URL: &str = "https://search.parallel.ai/mcp";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const DEFAULT_RESULTS: usize = 8;
const MAX_RESULTS: usize = 20;
const MAX_QUERY_CHARS: usize = 512;
static HTTP_CLIENT: OnceLock<Result<reqwest::Client, ()>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WebSearchProvider {
    Exa,
    Parallel,
}

impl WebSearchProvider {
    fn id(self) -> &'static str {
        match self {
            Self::Exa => "exa",
            Self::Parallel => "parallel",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::Exa => EXA_MCP_URL,
            Self::Parallel => PARALLEL_MCP_URL,
        }
    }
}

impl fmt::Display for WebSearchProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exa => formatter.write_str("Exa"),
            Self::Parallel => formatter.write_str("Parallel"),
        }
    }
}

#[derive(Debug, Clone)]
struct ValidatedQuery {
    query: String,
    limit: usize,
}

/// MCP response text returned to the LLM verbatim. Not parsed — see module docs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchResponse {
    pub query: String,
    pub provider: String,
    pub text: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum WebSearchError {
    #[error("{0}")]
    InvalidInput(&'static str),
    #[error("web_search could not initialize its HTTP client")]
    ClientBuild,
    #[error("web_search was rate-limited by {provider}; stop retrying and try again later")]
    RateLimited { provider: WebSearchProvider },
    #[error("web_search authentication was rejected by {provider}")]
    Authentication { provider: WebSearchProvider },
    #[error("web_search could not reach {provider}")]
    Unavailable { provider: WebSearchProvider },
    #[error("web_search received an invalid response from {provider}")]
    InvalidResponse { provider: WebSearchProvider },
    #[error("web_search response from {provider} exceeded the 256 KiB limit")]
    ResponseTooLarge { provider: WebSearchProvider },
}

fn build_query(
    input: &WebSearchInput,
    redact_rules: &[RedactRule],
) -> Result<ValidatedQuery, &'static str> {
    let raw_query = input.query.trim();
    if raw_query.is_empty() || raw_query.chars().count() > MAX_QUERY_CHARS {
        return Err("web_search query must be 1..=512 characters");
    }

    // This is the network egress boundary. Prompts can ask the model not to
    // copy secrets into a query, but only local redaction can enforce it.
    let redacted_query = sanitize::redact(raw_query, redact_rules);
    let query = redacted_query.trim();
    if query.is_empty() || query.chars().count() > MAX_QUERY_CHARS {
        return Err("web_search query must be 1..=512 characters");
    }

    let limit = input.max_results.unwrap_or(DEFAULT_RESULTS);
    if !(1..=MAX_RESULTS).contains(&limit) {
        return Err("web_search max_results must be between 1 and 20");
    }

    Ok(ValidatedQuery {
        query: query.to_owned(),
        limit,
    })
}

fn select_provider(routing_key: &str, provider_override: Option<&str>) -> WebSearchProvider {
    match provider_override {
        Some("exa") => return WebSearchProvider::Exa,
        Some("parallel") => return WebSearchProvider::Parallel,
        _ => {}
    }

    let hash = Sha256::digest(routing_key.as_bytes());
    if hash[0] % 2 == 0 {
        WebSearchProvider::Exa
    } else {
        WebSearchProvider::Parallel
    }
}

fn mcp_request(provider: WebSearchProvider, query: &ValidatedQuery) -> Value {
    let (name, arguments) = match provider {
        WebSearchProvider::Exa => (
            "web_search_exa",
            json!({
                "query": query.query,
                "type": "auto",
                "numResults": query.limit,
                "livecrawl": "fallback",
            }),
        ),
        WebSearchProvider::Parallel => (
            "web_search",
            json!({
                "objective": query.query,
                "search_queries": [query.query],
            }),
        ),
    };
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": name,
            "arguments": arguments,
        }
    })
}

#[derive(Debug, Deserialize)]
struct McpEnvelope {
    result: Option<McpResult>,
    error: Option<McpJsonError>,
}

#[derive(Debug, Deserialize)]
struct McpResult {
    #[serde(default)]
    content: Vec<McpContent>,
    #[serde(default, rename = "isError")]
    is_error: bool,
}

#[derive(Debug, Deserialize)]
struct McpContent {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct McpJsonError {
    message: String,
}

struct McpText {
    text: String,
    is_error: bool,
}

fn parse_mcp_payload(payload: &str) -> Option<McpText> {
    let envelope: McpEnvelope = serde_json::from_str(payload.trim()).ok()?;
    if let Some(error) = envelope.error {
        return Some(McpText {
            text: error.message,
            is_error: true,
        });
    }

    let result = envelope.result?;
    let text = result
        .content
        .into_iter()
        .filter_map(|content| content.text)
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(McpText {
        text,
        is_error: result.is_error,
    })
}

fn parse_mcp_response(body: &str) -> Option<McpText> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        if let Some(result) = parse_mcp_payload(trimmed) {
            return Some(result);
        }
    }

    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find_map(parse_mcp_payload)
}

fn is_rate_limit_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("rate limit") || lower.contains("too many requests") || lower.contains("429")
}

/// Decode the MCP envelope and return its text verbatim. A provider-side
/// `isError` is surfaced as an error (rate limits mapped specifically) so the
/// LLM learns the search failed rather than trusting an error body as data.
fn decode_search_text(provider: WebSearchProvider, body: &str) -> Result<String, WebSearchError> {
    let result = parse_mcp_response(body).ok_or(WebSearchError::InvalidResponse { provider })?;
    if result.is_error {
        return if is_rate_limit_error(&result.text) {
            Err(WebSearchError::RateLimited { provider })
        } else {
            Err(WebSearchError::Unavailable { provider })
        };
    }
    Ok(result.text)
}

fn http_client() -> Result<&'static reqwest::Client, WebSearchError> {
    HTTP_CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .user_agent(concat!("rssh/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|_| ())
        })
        .as_ref()
        .map_err(|_| WebSearchError::ClientBuild)
}

async fn read_response(
    response: reqwest::Response,
    provider: WebSearchProvider,
) -> Result<Vec<u8>, WebSearchError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(WebSearchError::ResponseTooLarge { provider });
    }

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| WebSearchError::Unavailable { provider })?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(WebSearchError::ResponseTooLarge { provider });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn search_provider(
    provider: WebSearchProvider,
    endpoint: &str,
    query: &ValidatedQuery,
) -> Result<WebSearchResponse, WebSearchError> {
    let started = Instant::now();
    let response = http_client()?
        .post(endpoint)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .json(&mcp_request(provider, query))
        .send()
        .await
        .map_err(|_| WebSearchError::Unavailable { provider })?;

    match response.status().as_u16() {
        200..=299 => {}
        401 | 403 => return Err(WebSearchError::Authentication { provider }),
        429 => return Err(WebSearchError::RateLimited { provider }),
        _ => return Err(WebSearchError::Unavailable { provider }),
    }

    let body = read_response(response, provider).await?;
    let body = String::from_utf8(body).map_err(|_| WebSearchError::InvalidResponse { provider })?;
    let text = decode_search_text(provider, &body)?;
    Ok(WebSearchResponse {
        // Never trust an upstream query echo for provenance. This is exactly
        // the locally redacted string sent across the network.
        query: query.query.clone(),
        provider: provider.id().to_owned(),
        text,
        elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    })
}

pub async fn search(
    input: &WebSearchInput,
    redact_rules: &[RedactRule],
    routing_key: &str,
) -> Result<WebSearchResponse, WebSearchError> {
    let query = build_query(input, redact_rules).map_err(WebSearchError::InvalidInput)?;
    let provider_override = std::env::var("RSSH_WEB_SEARCH_PROVIDER").ok();
    let provider = select_provider(routing_key, provider_override.as_deref());
    search_provider(provider, provider.endpoint(), &query).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;
    use serde_json::json;

    #[test]
    fn builds_a_bounded_redacted_search_query() {
        let query = build_query(
            &WebSearchInput {
                query: "  rust async patterns  ".into(),
                max_results: None,
            },
            &[],
        )
        .unwrap();

        assert_eq!(query.query, "rust async patterns");
        assert_eq!(query.limit, DEFAULT_RESULTS);
    }

    #[test]
    fn redacts_query_before_building_provider_query() {
        let rules =
            vec![
                super::super::sanitize::RedactRule::new(r"EMP-\d{4}", "<REDACTED:employee>")
                    .unwrap(),
            ];

        let query = build_query(
            &WebSearchInput {
                query: "debug deployment for EMP-1234".into(),
                max_results: None,
            },
            &rules,
        )
        .unwrap();

        assert_eq!(query.query, "debug deployment for <REDACTED:employee>");
        assert!(!query.query.contains("EMP-1234"));
    }

    #[test]
    fn rejects_invalid_query_and_limits() {
        let input = |query: String, max_results| WebSearchInput { query, max_results };

        assert!(build_query(&input("   ".into(), None), &[]).is_err());
        assert!(build_query(&input("界".repeat(MAX_QUERY_CHARS + 1), None), &[]).is_err());
        assert!(build_query(&input("rust".into(), Some(0)), &[]).is_err());
        assert!(build_query(&input("rust".into(), Some(MAX_RESULTS + 1)), &[]).is_err());
    }

    #[test]
    fn rejects_query_that_is_empty_after_redaction() {
        let rules = vec![super::super::sanitize::RedactRule::new(r"EMP-\d{4}", "").unwrap()];

        assert!(build_query(
            &WebSearchInput {
                query: "EMP-1234".into(),
                max_results: None,
            },
            &rules,
        )
        .is_err());
    }

    #[test]
    fn selects_a_provider_stably_and_honors_an_explicit_override() {
        assert_eq!(
            select_provider("conversation", Some("exa")),
            WebSearchProvider::Exa
        );
        assert_eq!(
            select_provider("conversation", Some("parallel")),
            WebSearchProvider::Parallel
        );
        assert_eq!(
            select_provider("conversation", None),
            select_provider("conversation", None)
        );

        let providers = (0..64)
            .map(|index| select_provider(&format!("conversation-{index}"), None))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(providers.len(), 2);
    }

    #[test]
    fn builds_provider_specific_mcp_calls_without_session_metadata() {
        let query = ValidatedQuery {
            query: "rust async patterns".into(),
            limit: 8,
        };

        let exa = mcp_request(WebSearchProvider::Exa, &query);
        assert_eq!(exa["method"], "tools/call");
        assert_eq!(exa["params"]["name"], "web_search_exa");
        assert_eq!(exa["params"]["arguments"]["query"], query.query);
        assert_eq!(exa["params"]["arguments"]["numResults"], query.limit);

        let parallel = mcp_request(WebSearchProvider::Parallel, &query);
        assert_eq!(parallel["params"]["name"], "web_search");
        assert_eq!(
            parallel["params"]["arguments"]["search_queries"],
            json!([query.query])
        );
        assert!(parallel["params"]["arguments"].get("session_id").is_none());
        assert!(parallel["params"]["arguments"].get("model_name").is_none());
    }

    #[test]
    fn decodes_mcp_text_verbatim_from_an_sse_response() {
        let provider_text = "Title: Rust\nURL: https://www.rust-lang.org/\nA language";
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "content": [{ "type": "text", "text": provider_text }],
                "isError": false
            }
        });
        let body = format!("event: message\ndata: {payload}\n\n");

        let text = decode_search_text(WebSearchProvider::Exa, &body).unwrap();

        assert_eq!(text, provider_text);
    }

    #[tokio::test]
    async fn calls_parallel_anonymously_and_returns_mcp_text() {
        let provider_text = "Title: Rust\nURL: https://www.rust-lang.org/";
        let response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "content": [{ "type": "text", "text": provider_text }],
                "isError": false
            }
        });
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_header("authorization", Matcher::Missing)
            .match_header("accept", "application/json, text/event-stream")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(response.to_string())
            .create_async()
            .await;
        let query = ValidatedQuery {
            query: "rust".into(),
            limit: 8,
        };

        let response = search_provider(WebSearchProvider::Parallel, &server.url(), &query)
            .await
            .unwrap();

        mock.assert_async().await;
        assert_eq!(response.provider, "parallel");
        assert_eq!(response.query, "rust");
        assert_eq!(response.text, provider_text);
    }

    #[tokio::test]
    async fn maps_rate_limits_without_leaking_the_response_body() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(429)
            .with_body("proxy password=secret")
            .create_async()
            .await;
        let query = ValidatedQuery {
            query: "rust".into(),
            limit: 8,
        };

        let error = search_provider(WebSearchProvider::Parallel, &server.url(), &query)
            .await
            .unwrap_err();

        mock.assert_async().await;
        assert!(matches!(
            error,
            WebSearchError::RateLimited {
                provider: WebSearchProvider::Parallel
            }
        ));
        assert!(!error.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn rejects_oversized_mcp_responses_before_reading_the_body() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(vec![b'x'; MAX_RESPONSE_BYTES + 1])
            .create_async()
            .await;
        let query = ValidatedQuery {
            query: "rust".into(),
            limit: 8,
        };

        let error = search_provider(WebSearchProvider::Parallel, &server.url(), &query)
            .await
            .unwrap_err();

        mock.assert_async().await;
        assert!(
            matches!(
                error,
                WebSearchError::ResponseTooLarge {
                    provider: WebSearchProvider::Parallel
                }
            ),
            "unexpected error: {error:?}"
        );
    }
}
