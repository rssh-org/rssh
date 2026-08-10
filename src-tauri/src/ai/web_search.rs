//! Bounded, zero-configuration public web search for AI sessions.

use h2m_search::{DuckDuckGo, SafeSearch, SearchError, SearchQuery, SearchResponse, TimeRange};
use serde::Serialize;
use url::Url;

use super::{
    sanitize::{self, RedactRule},
    tools::WebSearchInput,
};

const DEFAULT_RESULTS: usize = 5;
const MAX_RESULTS: usize = 10;
const MAX_QUERY_CHARS: usize = 512;
const MAX_TITLE_CHARS: usize = 300;
const MAX_SNIPPET_CHARS: usize = 1_000;
const MAX_URL_CHARS: usize = 2_048;

#[derive(Debug, Clone, Serialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebSearchResponse {
    pub query: String,
    pub provider: String,
    pub results: Vec<WebSearchResult>,
    pub elapsed_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum WebSearchError {
    #[error("{0}")]
    InvalidInput(&'static str),
    #[error("web_search could not initialize its HTTP client")]
    ClientBuild,
    #[error("web_search was rate-limited by DuckDuckGo; stop retrying and try again later")]
    RateLimited,
    #[error("web_search was blocked by a DuckDuckGo CAPTCHA; stop retrying and try again later")]
    Captcha,
    #[error("web_search could not reach DuckDuckGo")]
    Unavailable,
    #[error("web_search received an invalid response from DuckDuckGo")]
    InvalidResponse,
}

fn build_query(
    input: &WebSearchInput,
    redact_rules: &[RedactRule],
) -> Result<SearchQuery, &'static str> {
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
        return Err("web_search max_results must be between 1 and 10");
    }

    let time_range = match input.freshness.as_deref() {
        None => None,
        Some("day") => Some(TimeRange::Day),
        Some("week") => Some(TimeRange::Week),
        Some("month") => Some(TimeRange::Month),
        Some("year") => Some(TimeRange::Year),
        Some(_) => {
            return Err("web_search freshness must be one of: day, week, month, year");
        }
    };

    let mut search = SearchQuery::new(query)
        .with_limit(limit)
        .with_safesearch(SafeSearch::Moderate);
    if let Some(range) = time_range {
        search = search.with_time_range(range);
    }
    Ok(search)
}

fn normalize_response(
    response: SearchResponse,
    limit: usize,
    sent_query: String,
) -> WebSearchResponse {
    let provider = response.provider.to_string();
    let elapsed_ms = response.elapsed_ms;
    let results = response
        .web
        .into_iter()
        .filter_map(|hit| {
            let raw_url = hit.url.trim();
            if raw_url.chars().count() > MAX_URL_CHARS {
                return None;
            }
            let url = Url::parse(raw_url).ok()?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return None;
            }

            let snippet = hit.description.and_then(|value| {
                let value = truncate_chars(&value, MAX_SNIPPET_CHARS);
                (!value.is_empty()).then_some(value)
            });
            Some(WebSearchResult {
                title: truncate_chars(&hit.title, MAX_TITLE_CHARS),
                url: raw_url.to_string(),
                snippet,
            })
        })
        .take(limit)
        .collect();

    WebSearchResponse {
        // Do not trust an upstream echo for local query provenance. This is
        // exactly the redacted string sent across the network.
        query: sent_query,
        provider,
        results,
        elapsed_ms,
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.trim().chars().take(max_chars).collect()
}

fn map_search_error(error: SearchError) -> WebSearchError {
    match error {
        SearchError::RateLimited { .. } => WebSearchError::RateLimited,
        SearchError::CaptchaDetected { .. } => WebSearchError::Captcha,
        SearchError::InvalidResponse { .. } | SearchError::ParseFailed { .. } => {
            WebSearchError::InvalidResponse
        }
        SearchError::Config { .. } => WebSearchError::ClientBuild,
        _ => WebSearchError::Unavailable,
    }
}

pub async fn search(
    input: &WebSearchInput,
    redact_rules: &[RedactRule],
) -> Result<WebSearchResponse, WebSearchError> {
    let query = build_query(input, redact_rules).map_err(WebSearchError::InvalidInput)?;
    let limit = query.limit;
    let sent_query = query.query.clone();
    let provider = DuckDuckGo::new().map_err(map_search_error)?;
    let response = provider.search(&query).await.map_err(map_search_error)?;
    Ok(normalize_response(response, limit, sent_query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_bounded_moderate_safe_search_query() {
        let query = build_query(
            &WebSearchInput {
                query: "  rust async patterns  ".into(),
                max_results: None,
                freshness: Some("week".into()),
            },
            &[],
        )
        .unwrap();

        assert_eq!(query.query, "rust async patterns");
        assert_eq!(query.limit, DEFAULT_RESULTS);
        assert_eq!(query.time_range, Some(TimeRange::Week));
        assert_eq!(query.safesearch, SafeSearch::Moderate);
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
                freshness: None,
            },
            &rules,
        )
        .unwrap();

        assert_eq!(query.query, "debug deployment for <REDACTED:employee>");
        assert!(!query.query.contains("EMP-1234"));
    }

    #[test]
    fn rejects_invalid_query_limits_and_freshness() {
        let input = |query: String, max_results, freshness| WebSearchInput {
            query,
            max_results,
            freshness,
        };

        assert!(build_query(&input("   ".into(), None, None), &[]).is_err());
        assert!(build_query(&input("界".repeat(MAX_QUERY_CHARS + 1), None, None), &[]).is_err());
        assert!(build_query(&input("rust".into(), Some(0), None), &[]).is_err());
        assert!(build_query(&input("rust".into(), Some(MAX_RESULTS + 1), None), &[]).is_err());
        assert!(build_query(&input("rust".into(), None, Some("hour".into())), &[]).is_err());
    }

    #[test]
    fn rejects_query_that_is_empty_after_redaction() {
        let rules = vec![super::super::sanitize::RedactRule::new(r"EMP-\d{4}", "").unwrap()];

        assert!(build_query(
            &WebSearchInput {
                query: "EMP-1234".into(),
                max_results: None,
                freshness: None,
            },
            &rules,
        )
        .is_err());
    }

    #[test]
    fn maps_every_supported_freshness_value() {
        for (raw, expected) in [
            ("day", TimeRange::Day),
            ("week", TimeRange::Week),
            ("month", TimeRange::Month),
            ("year", TimeRange::Year),
        ] {
            let query = build_query(
                &WebSearchInput {
                    query: "rust".into(),
                    max_results: Some(MAX_RESULTS),
                    freshness: Some(raw.into()),
                },
                &[],
            )
            .unwrap();

            assert_eq!(query.time_range, Some(expected));
        }
    }

    #[test]
    fn returns_only_bounded_http_results_with_utf8_safe_text() {
        let mut upstream = SearchResponse::new("untrusted upstream echo", "duckduckgo");
        upstream.web.push(
            h2m_search::SearchHit::new("界".repeat(MAX_TITLE_CHARS + 1), "https://a.example/")
                .with_description("文".repeat(MAX_SNIPPET_CHARS + 1)),
        );
        upstream
            .web
            .push(h2m_search::SearchHit::new("unsafe", "javascript:alert(1)"));
        upstream.web.push(h2m_search::SearchHit::new(
            "too long",
            format!("https://b.example/{}", "x".repeat(MAX_URL_CHARS)),
        ));
        upstream.web.push(h2m_search::SearchHit::new(
            "plain http",
            "http://c.example/",
        ));

        let result = normalize_response(upstream, 2, "rust".into());

        assert_eq!(result.query, "rust");
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.results[0].title.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(
            result.results[0].snippet.as_ref().unwrap().chars().count(),
            MAX_SNIPPET_CHARS
        );
        assert_eq!(result.results[1].url, "http://c.example/");
    }

    #[test]
    fn maps_provider_errors_without_leaking_raw_details() {
        let error = map_search_error(SearchError::Transport {
            provider: "duckduckgo",
            message: "proxy password=secret".into(),
        });

        assert_eq!(error.to_string(), "web_search could not reach DuckDuckGo");
        assert!(!error.to_string().contains("secret"));
    }
}
