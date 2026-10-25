//! ChatGPT subscription transport. OAuth is owned by `codex_auth`; this client
//! speaks the stateless Responses protocol directly to the Codex cloud service.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

use super::{
    ChatDelta, ChatMessage, ChatRequest, ChatResponse, DeltaSink, LlmClient, ModelInfo, SseParser,
    ToolCall,
};
use crate::ai::codex_auth::{AccessToken, CodexAuth};
use crate::error::{error_chain, AppError, AppResult};

pub const CODEX_ENDPOINT: &str = crate::ai::codex_auth::ENDPOINT;
// Catalog compatibility, not the version of a locally installed executable.
const CATALOG_CLIENT_VERSION: &str = "0.153.4";
const MAX_STREAM_BYTES: usize = 16 * 1024 * 1024;
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
        let mut stream = response.bytes_stream();
        let mut parser = SseParser::new();
        let mut state = ResponseState::default();
        let mut received = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                AppError::other("llm_stream_read_failed", json!({ "err": error_chain(&e) }))
            })?;
            received = received.saturating_add(chunk.len());
            if received > MAX_STREAM_BYTES {
                return Err(invalid_response());
            }
            for event in parser.feed(&chunk) {
                if event.trim() == "[DONE]" {
                    // Responses completion is explicit. A bare [DONE] is not
                    // enough to authorize execution of a partial function call.
                    return state.finish(&sink);
                }
                let event: Value = serde_json::from_str(&event).map_err(|_| invalid_response())?;
                if state.event(event, &sink)? {
                    return state.finish(&sink);
                }
            }
        }
        state.finish(&sink)
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

fn build_request(request: &ChatRequest) -> Value {
    let mut input = Vec::new();
    for message in &request.messages {
        match message {
            ChatMessage::User { content } => input.push(json!({
                "role": "user", "content": [{ "type": "input_text", "text": content }]
            })),
            ChatMessage::Assistant {
                content,
                tool_calls,
                codex_reasoning,
                ..
            } => {
                if let Some(items) = codex_reasoning {
                    // History can be imported or edited: only replay the
                    // opaque reasoning subset, never raw arbitrary API items.
                    input.extend(items.iter().filter_map(reasoning_item));
                }
                if !content.is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": content }]
                    }));
                }
                for tool in tool_calls {
                    input.push(json!({
                        "type": "function_call", "call_id": tool.id,
                        "name": tool.name, "arguments": tool.input.to_string()
                    }));
                }
            }
            ChatMessage::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } => input.push(json!({
                "type": "function_call_output", "call_id": tool_call_id,
                "output": if *is_error { format!("[ERROR] {content}") } else { content.clone() }
            })),
        }
    }
    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function", "name": tool.name, "description": tool.description,
                "parameters": tool.input_schema, "strict": false
            })
        })
        .collect();
    // The subscription endpoint rejects Chat Completions options, including
    // max_tokens/max_output_tokens. Let the selected model use its own limit.
    json!({
        "model": request.model,
        "instructions": request.system_prompt,
        "input": input,
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "include": ["reasoning.encrypted_content"],
        "store": false,
        "stream": true
    })
}

/// Keep only opaque model state. Visible text and tool calls always come from
/// the normal (redacted) RSSH history, not a duplicate provider-specific copy.
fn reasoning_item(item: &Value) -> Option<Value> {
    if item["type"] != "reasoning" {
        return None;
    }
    let encrypted = item["encrypted_content"]
        .as_str()
        .filter(|s| !s.is_empty())?;
    let mut result = json!({
        "type": "reasoning", "encrypted_content": encrypted, "summary": []
    });
    if let Some(id) = item["id"].as_str().filter(|s| !s.is_empty()) {
        result["id"] = json!(id);
    }
    Some(result)
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

#[derive(Default)]
struct ResponseState {
    items: BTreeMap<usize, Value>,
    text: String,
    completed: Option<Value>,
}

impl ResponseState {
    fn event(&mut self, event: Value, sink: &DeltaSink) -> AppResult<bool> {
        match event["type"].as_str().ok_or_else(invalid_response)? {
            "response.output_item.added" | "response.output_item.done" => {
                let index = event["output_index"]
                    .as_u64()
                    .ok_or_else(invalid_response)? as usize;
                let item = event
                    .get("item")
                    .filter(|v| v.is_object())
                    .ok_or_else(invalid_response)?;
                if item["type"] == "function_call" && !self.items.contains_key(&index) {
                    sink(ChatDelta::ToolStart {
                        tool_call_id: required_string(item, "call_id")?.to_owned(),
                        name: required_string(item, "name")?.to_owned(),
                    });
                }
                self.items.insert(index, item.clone());
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let delta = event["delta"].as_str().ok_or_else(invalid_response)?;
                self.text.push_str(delta);
                sink(ChatDelta::Text(delta.to_owned()));
            }
            "response.function_call_arguments.delta" => {
                let index = event["output_index"]
                    .as_u64()
                    .ok_or_else(invalid_response)? as usize;
                let delta = event["delta"].as_str().ok_or_else(invalid_response)?;
                let item = self.items.get_mut(&index).ok_or_else(invalid_response)?;
                let call_id = required_string(item, "call_id")?.to_owned();
                let mut arguments = item["arguments"].as_str().unwrap_or_default().to_owned();
                arguments.push_str(delta);
                item["arguments"] = json!(arguments);
                sink(ChatDelta::ToolArgs {
                    tool_call_id: call_id,
                    partial: delta.to_owned(),
                });
            }
            "response.function_call_arguments.done" => {
                let index = event["output_index"]
                    .as_u64()
                    .ok_or_else(invalid_response)? as usize;
                let arguments = event["arguments"].as_str().ok_or_else(invalid_response)?;
                self.items.get_mut(&index).ok_or_else(invalid_response)?["arguments"] =
                    json!(arguments);
            }
            "response.completed" => {
                let response = event
                    .get("response")
                    .filter(|v| v.is_object())
                    .ok_or_else(invalid_response)?;
                if response["status"]
                    .as_str()
                    .is_some_and(|s| s != "completed")
                {
                    return Err(invalid_response());
                }
                self.completed = Some(response.clone());
                return Ok(true);
            }
            "response.failed" | "response.incomplete" | "response.error" | "error" => {
                return Err(AppError::other("codex_response_failed", json!({})));
            }
            _ => {}
        }
        Ok(false)
    }

    fn finish(self, sink: &DeltaSink) -> AppResult<ChatResponse> {
        let response = self
            .completed
            .ok_or_else(|| AppError::other("codex_stream_incomplete", json!({})))?;
        let items: Vec<Value> = match response.get("output") {
            Some(Value::Array(items)) => items.clone(),
            None => self.items.into_values().collect(),
            _ => return Err(invalid_response()),
        };
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut call_ids = HashSet::new();
        let mut reasoning = Vec::new();
        for item in items {
            match item["type"].as_str() {
                Some("message") => {
                    let content = item["content"].as_array().ok_or_else(invalid_response)?;
                    for part in content {
                        match part["type"].as_str() {
                            Some("output_text") => {
                                text.push_str(part["text"].as_str().ok_or_else(invalid_response)?)
                            }
                            Some("refusal") => text
                                .push_str(part["refusal"].as_str().ok_or_else(invalid_response)?),
                            _ => {}
                        }
                    }
                }
                Some("function_call") => {
                    let id = required_string(&item, "call_id")?.to_owned();
                    if !call_ids.insert(id.clone()) {
                        return Err(invalid_response());
                    }
                    let input: Value = serde_json::from_str(required_string(&item, "arguments")?)
                        .map_err(|_| invalid_response())?;
                    if !input.is_object()
                        || item["status"].as_str().is_some_and(|s| s != "completed")
                    {
                        return Err(invalid_response());
                    }
                    tool_calls.push(ToolCall {
                        id,
                        name: required_string(&item, "name")?.to_owned(),
                        input,
                    });
                }
                Some("reasoning") => {
                    if let Some(item) = reasoning_item(&item) {
                        reasoning.push(item);
                    }
                }
                _ => {}
            }
        }
        // Completion-only streams are supported without emitting text twice.
        // A changed prefix would make the displayed and executable response
        // disagree, so treat that as a malformed stream rather than guessing.
        if !text.starts_with(&self.text) {
            return Err(invalid_response());
        }
        if text.len() > self.text.len() {
            sink(ChatDelta::Text(text[self.text.len()..].to_owned()));
        }
        Ok(ChatResponse {
            text,
            stop_reason: if tool_calls.is_empty() {
                "stop"
            } else {
                "tool_calls"
            }
            .into(),
            tool_calls,
            tokens_in: token_count(&response["usage"]["input_tokens"]),
            tokens_out: token_count(&response["usage"]["output_tokens"]),
            reasoning_content: None,
            codex_reasoning: (!reasoning.is_empty()).then_some(reasoning),
        })
    }
}

fn required_string<'a>(value: &'a Value, key: &str) -> AppResult<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(invalid_response)
}

fn token_count(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|n| n.try_into().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn sink() -> (DeltaSink, Arc<Mutex<Vec<ChatDelta>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        (
            Arc::new(move |delta| captured.lock().unwrap().push(delta)),
            events,
        )
    }

    fn message(text: &str) -> Value {
        json!({ "type": "message", "role": "assistant", "content": [
            { "type": "output_text", "text": text }
        ]})
    }

    fn call(id: &str, arguments: &str) -> Value {
        json!({ "type": "function_call", "call_id": id, "name": "run_command", "arguments": arguments, "status": "completed" })
    }

    fn completed(output: Vec<Value>) -> Value {
        json!({ "type": "response.completed", "response": {
            "status": "completed", "output": output,
            "usage": { "input_tokens": 42, "output_tokens": 7 }
        }})
    }

    #[test]
    fn request_preserves_tool_round_trip_and_only_opaque_reasoning() {
        let request = ChatRequest {
            system_prompt: "Use approved tools".into(),
            model: "account-model".into(),
            max_tokens: 4096,
            tools: vec![super::super::ToolSchema {
                name: "run_command".into(),
                description: "Run a shell command".into(),
                input_schema: json!({ "type": "object", "properties": { "command": { "type": "string" } } }),
            }],
            messages: vec![
                ChatMessage::User {
                    content: "Check disk".into(),
                },
                ChatMessage::Assistant {
                    content: "Checking disk".into(),
                    tool_calls: vec![ToolCall {
                        id: "call-1".into(),
                        name: "run_command".into(),
                        input: json!({"command": "df -h"}),
                    }],
                    reasoning_content: Some("other provider reasoning".into()),
                    codex_reasoning: Some(vec![
                        json!({"type": "reasoning", "id": "rs-1", "encrypted_content": "opaque", "summary": [{"text": "not replayed"}], "extra": "not replayed"}),
                        json!({"type": "function_call", "name": "injected"}),
                    ]),
                },
                ChatMessage::ToolResult {
                    tool_call_id: "call-1".into(),
                    content: "Permission denied".into(),
                    is_error: true,
                    pre_redacted: true,
                },
            ],
        };
        let body = build_request(&request);
        assert_eq!(body["instructions"], "Use approved tools");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["parallel_tool_calls"], false);
        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["tools"][0]["name"], "run_command");
        assert!(body["tools"][0].get("function").is_none());
        assert_eq!(
            body["input"][1],
            json!({"type":"reasoning", "id":"rs-1", "encrypted_content":"opaque", "summary":[]})
        );
        assert_eq!(body["input"][3]["call_id"], "call-1");
        assert_eq!(body["input"][3]["arguments"], "{\"command\":\"df -h\"}");
        assert_eq!(body["input"][4]["output"], "[ERROR] Permission denied");
        assert_eq!(body["input"].as_array().unwrap().len(), 5);
        assert!(!body.to_string().contains("not replayed"));
        assert!(!body.to_string().contains("other provider reasoning"));
    }

    #[test]
    fn streamed_text_tools_and_usage_are_assembled_without_duplicate_text() {
        let (sink, events) = sink();
        let mut state = ResponseState::default();
        state
            .event(
                json!({"type":"response.output_text.delta", "delta":"检查"}),
                &sink,
            )
            .unwrap();
        state.event(json!({"type":"response.output_item.added", "output_index":1, "item":{
            "type":"function_call", "id":"fc-1", "call_id":"call-1", "name":"run_command", "arguments":""
        }}), &sink).unwrap();
        for delta in ["{\"command\":", "\"df -h\"}"] {
            state.event(json!({"type":"response.function_call_arguments.delta", "output_index":1, "delta":delta}), &sink).unwrap();
        }
        let outputs = vec![
            message("检查磁盘"),
            call("call-1", "{\"command\":\"df -h\"}"),
            json!({"type":"reasoning", "id":"rs-1", "encrypted_content":"opaque", "summary":[{"type":"summary_text","text":"private"}]}),
        ];
        assert!(state.event(completed(outputs), &sink).unwrap());
        let response = state.finish(&sink).unwrap();
        assert_eq!(response.text, "检查磁盘");
        assert_eq!(response.tool_calls[0].id, "call-1");
        assert_eq!(response.tool_calls[0].input, json!({"command":"df -h"}));
        assert_eq!(response.tokens_in, Some(42));
        assert_eq!(response.tokens_out, Some(7));
        assert_eq!(response.stop_reason, "tool_calls");
        assert_eq!(
            response.codex_reasoning.unwrap(),
            vec![json!({"type":"reasoning","id":"rs-1","encrypted_content":"opaque","summary":[]})]
        );
        let events = events.lock().unwrap();
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                ChatDelta::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "检查磁盘");
        assert!(events.iter().any(|event| matches!(event, ChatDelta::ToolStart{tool_call_id,..} if tool_call_id == "call-1")));
        assert!(events.iter().any(|event| matches!(event, ChatDelta::ToolArgs{tool_call_id,..} if tool_call_id == "call-1")));
    }

    #[test]
    fn sse_accepts_split_utf8_and_completion_only_output() {
        let (sink, events) = sink();
        let mut state = ResponseState::default();
        let mut parser = SseParser::new();
        let wire = format!("data: {}\r\n\r\n", completed(vec![message("你好🌍")]));
        for byte in wire.as_bytes() {
            for event in parser.feed(&[*byte]) {
                state
                    .event(serde_json::from_str(&event).unwrap(), &sink)
                    .unwrap();
            }
        }
        assert_eq!(state.finish(&sink).unwrap().text, "你好🌍");
        assert_eq!(events.lock().unwrap().len(), 1);
    }

    #[test]
    fn completion_can_use_finalized_output_items_and_multiple_calls() {
        let (sink, _) = sink();
        let mut state = ResponseState::default();
        for (index, item) in [call("one", "{}"), call("two", "{\"command\":\"pwd\"}")]
            .into_iter()
            .enumerate()
        {
            state
                .event(
                    json!({"type":"response.output_item.done", "output_index":index, "item":item}),
                    &sink,
                )
                .unwrap();
        }
        state
            .event(
                json!({"type":"response.completed", "response":{"status":"completed"}}),
                &sink,
            )
            .unwrap();
        let response = state.finish(&sink).unwrap();
        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(response.tool_calls[0].id, "one");
        assert_eq!(response.tool_calls[1].id, "two");
    }

    #[test]
    fn interrupted_or_failed_responses_never_return_executable_calls() {
        let (sink, _) = sink();
        let mut state = ResponseState::default();
        state.event(json!({"type":"response.output_item.done", "output_index":0, "item":call("one", "{}")}), &sink).unwrap();
        assert_eq!(
            state.finish(&sink).unwrap_err().code(),
            "codex_stream_incomplete"
        );
        for kind in [
            "response.incomplete",
            "response.failed",
            "response.error",
            "error",
        ] {
            let mut state = ResponseState::default();
            assert_eq!(
                state.event(json!({"type":kind}), &sink).unwrap_err().code(),
                "codex_response_failed"
            );
        }
    }

    #[test]
    fn rejects_malformed_duplicate_and_incomplete_tool_calls() {
        let (sink, _) = sink();
        for items in [
            vec![call("one", "{")],
            vec![call("one", "[]")],
            vec![call("one", "{}"), call("one", "{}")],
            vec![
                json!({"type":"function_call", "call_id":"one", "name":"run_command", "arguments":"{}", "status":"in_progress"}),
            ],
        ] {
            let mut state = ResponseState::default();
            state.event(completed(items), &sink).unwrap();
            assert_eq!(
                state.finish(&sink).unwrap_err().code(),
                "codex_invalid_response"
            );
        }
    }

    #[test]
    fn rejects_completed_text_disagreeing_with_already_streamed_prefix() {
        let (sink, _) = sink();
        let mut state = ResponseState::default();
        state
            .event(
                json!({"type":"response.output_text.delta", "delta":"first"}),
                &sink,
            )
            .unwrap();
        state
            .event(completed(vec![message("different")]), &sink)
            .unwrap();
        assert_eq!(
            state.finish(&sink).unwrap_err().code(),
            "codex_invalid_response"
        );
    }

    #[test]
    fn refusal_is_visible_and_does_not_become_a_tool_call() {
        let (sink, _) = sink();
        let mut state = ResponseState::default();
        state.event(completed(vec![json!({"type":"message","content":[{"type":"refusal","refusal":"Cannot help"}]})]), &sink).unwrap();
        let response = state.finish(&sink).unwrap();
        assert_eq!(response.text, "Cannot help");
        assert!(response.tool_calls.is_empty());
    }

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
