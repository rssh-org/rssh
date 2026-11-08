//! ChatGPT subscription transport. OAuth is owned by `codex_auth`; this client
//! speaks the stateless Responses protocol directly to the Codex cloud service.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

use super::responses::{build_request, read_response};
use super::{ChatRequest, ChatResponse, DeltaSink, LlmClient, ModelInfo};
use crate::ai::codex_auth::{AccessToken, CodexAuth};
use crate::error::{error_chain, AppError, AppResult};

pub const CODEX_ENDPOINT: &str = crate::ai::codex_auth::ENDPOINT;
// Catalog compatibility, not the version of a locally installed executable.
const CATALOG_CLIENT_VERSION: &str = "0.153.4";
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;

pub struct CodexClient {
    provider_id: String,
    auth: Arc<CodexAuth>,
    http: reqwest::Client,
}

impl CodexClient {
    pub fn new(provider_id: String, auth: Arc<CodexAuth>) -> AppResult<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("rssh/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(request_error)?;
        Ok(Self {
            provider_id,
            auth,
            http,
        })
    }

    fn headers(
        &self,
        request: reqwest::RequestBuilder,
        token: &AccessToken,
    ) -> reqwest::RequestBuilder {
        request
            .bearer_auth(&token.access_token)
            .header("chatgpt-account-id", &token.account_id)
            .header("originator", "rssh")
            .header("openai-beta", "responses=experimental")
    }

    /// A rejected access token may be stale because another request already
    /// refreshed it. The auth manager handles that comparison under its lock.
    /// Retry only a 401 response, before any streamed output has been consumed.
    async fn send(&self, body: Option<&Value>) -> AppResult<reqwest::Response> {
        let mut token = self.auth.authorize(&self.provider_id, None).await?;
        for attempt in 0..2 {
            let request = match body {
                Some(body) => self
                    .http
                    .post(format!("{CODEX_ENDPOINT}/responses"))
                    .header("accept", "text/event-stream")
                    .json(body),
                None => self
                    .http
                    .get(format!("{CODEX_ENDPOINT}/models"))
                    .query(&[("client_version", CATALOG_CLIENT_VERSION)])
                    .header("accept", "application/json")
                    .timeout(Duration::from_secs(60)),
            };
            let response = self
                .headers(request, &token)
                .send()
                .await
                .map_err(request_error)?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                drop(response);
                token = self
                    .auth
                    .authorize(&self.provider_id, Some(&token.access_token))
                    .await?;
                continue;
            }
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                self.auth
                    .reject(&self.provider_id, &token.access_token)
                    .await?;
                return Err(AppError::config("codex_login_required", json!({})));
            }
            if !response.status().is_success() {
                let status = response.status();
                // Do not return arbitrary response bodies containing credentials
                // or prompts. Extract only the provider's structured error text.
                let bytes = read_bounded(response, 16 * 1024).await?;
                let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let message = error_message(&body).replace(&token.access_token, "[redacted]");
                return Err(AppError::other(
                    "llm_error_status",
                    json!({ "status": status.to_string(), "text": message }),
                ));
            }
            return Ok(response);
        }
        unreachable!("the second request always returns")
    }
}

#[async_trait]
impl LlmClient for CodexClient {
    async fn chat(&self, request: ChatRequest, sink: DeltaSink) -> AppResult<ChatResponse> {
        let response = self.send(Some(&build_request(&request))).await?;
        read_response(response, sink).await
    }

    async fn list_models(&self) -> AppResult<Vec<ModelInfo>> {
        let response = self.send(None).await?;
        let bytes = read_bounded(response, MAX_JSON_BYTES).await?;
        let body: Value = serde_json::from_slice(&bytes).map_err(|_| invalid_response())?;
        parse_models(body)
    }
}

fn request_error(error: reqwest::Error) -> AppError {
    AppError::other("llm_request_failed", json!({ "err": error_chain(&error) }))
}

fn invalid_response() -> AppError {
    AppError::other("codex_invalid_response", json!({}))
}

fn error_message(body: &Value) -> String {
    body.pointer("/error/message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(2048)
        .collect()
}

async fn read_bounded(response: reqwest::Response, limit: usize) -> AppResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(request_error)?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(invalid_response());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_models(body: Value) -> AppResult<Vec<ModelInfo>> {
    let mut entries: Vec<&Value> = body["models"]
        .as_array()
        .ok_or_else(invalid_response)?
        .iter()
        .filter(|model| model["visibility"].as_str().is_none_or(|v| v == "list"))
        .collect();
    entries.sort_by_key(|model| model["priority"].as_i64().unwrap_or(i64::MAX));
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for entry in entries {
        let id = entry["slug"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid_response)?;
        if seen.insert(id) {
            models.push(ModelInfo {
                id: id.to_owned(),
                display_name: entry["display_name"].as_str().map(str::to_owned),
            });
        }
    }
    if models.is_empty() {
        return Err(AppError::other("codex_models_empty", json!({})));
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_only_visible_account_models_in_catalog_order() {
        let models = parse_models(json!({"models":[
            {"slug":"second","display_name":"Second", "visibility":"list","priority":2},
            {"slug":"hidden","visibility":"hide","priority":0},
            {"slug":"first","visibility":"list","priority":1},
            {"slug":"second","visibility":"list","priority":3}
        ]}))
        .unwrap();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert_eq!(models[1].display_name.as_deref(), Some("Second"));
        assert_eq!(
            parse_models(json!({"models":[]})).unwrap_err().code(),
            "codex_models_empty"
        );
        assert_eq!(
            parse_models(json!({"data":[]})).unwrap_err().code(),
            "codex_invalid_response"
        );
    }
}
