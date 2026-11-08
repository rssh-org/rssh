//! Stateless Responses request encoding and streamed response assembly, shared
//! by the OpenAI API-key and ChatGPT subscription transports.

use std::collections::{BTreeMap, HashSet};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

use super::{
    ChatDelta, ChatMessage, ChatRequest, ChatResponse, CodexAssistantMessage, DeltaSink, LlmClient,
    ModelInfo, SseParser, ToolCall,
};
use crate::error::{error_chain, AppError, AppResult};

const MAX_STREAM_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct OpenAiResponsesClient {
    api_key: String,
    endpoint: String,
    http: reqwest::Client,
}

impl OpenAiResponsesClient {
    pub(super) fn new(api_key: String, endpoint: String) -> Self {
        let endpoint = endpoint.trim().trim_end_matches('/');
        let endpoint = if endpoint.ends_with("/responses") {
            endpoint.to_owned()
        } else {
            format!("{endpoint}/responses")
        };
        Self {
            api_key,
            endpoint,
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmClient for OpenAiResponsesClient {
    async fn chat(&self, request: ChatRequest, sink: DeltaSink) -> AppResult<ChatResponse> {
        let mut body = build_request(&request);
        body["max_output_tokens"] = json!(request.max_tokens);
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::other("llm_request_failed", json!({"err": error_chain(&e)})))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::other(
                "llm_error_status",
                json!({"status": status.to_string(), "text": text}),
            ));
        }
        read_response(response, sink).await
    }

    async fn list_models(&self) -> AppResult<Vec<ModelInfo>> {
        let base = self.endpoint.strip_suffix("/responses").unwrap();
        super::completions::list_models(&self.http, &format!("{base}/models"), &self.api_key).await
    }
}

pub(super) async fn read_response(
    response: reqwest::Response,
    sink: DeltaSink,
) -> AppResult<ChatResponse> {
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

fn invalid_response() -> AppError {
    AppError::other("llm_invalid_response", json!({}))
}

pub(super) fn build_request(request: &ChatRequest) -> Value {
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
                codex_messages,
                ..
            } => {
                if let Some(items) = codex_reasoning {
                    // History can be imported or edited: only replay the
                    // opaque reasoning subset, never raw arbitrary API items.
                    input.extend(items.iter().filter_map(reasoning_item));
                }
                if let Some(messages) = codex_messages {
                    input.extend(messages.iter().map(|message| {
                        assistant_message_item(&message.content, message.phase.as_deref())
                    }));
                } else if !content.is_empty() {
                    // Old conversations and other providers have no phase.
                    input.push(assistant_message_item(content, None));
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
    // Output limits are transport-specific: the subscription endpoint rejects
    // max_output_tokens, while the API-key transport can add it.
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

fn assistant_message_item(content: &str, phase: Option<&str>) -> Value {
    let mut item = json!({
        "role": "assistant",
        "content": [{ "type": "output_text", "text": content }]
    });
    if let Some(phase) = phase {
        item["phase"] = json!(phase);
    }
    item
}

fn parse_assistant_message(item: &Value) -> AppResult<CodexAssistantMessage> {
    let mut content = String::new();
    for part in item["content"].as_array().ok_or_else(invalid_response)? {
        let field = match part["type"].as_str() {
            Some("output_text") => "text",
            Some("refusal") => "refusal",
            _ => continue,
        };
        content.push_str(part[field].as_str().ok_or_else(invalid_response)?);
    }
    Ok(CodexAssistantMessage {
        content,
        phase: item["phase"].as_str().map(str::to_owned),
    })
}

/// Keep only opaque model state here. Visible text and tool calls are replayed
/// separately from the redacted history fields.
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
                return Err(AppError::other("llm_response_failed", json!({})));
            }
            _ => {}
        }
        Ok(false)
    }

    fn finish(self, sink: &DeltaSink) -> AppResult<ChatResponse> {
        let response = self
            .completed
            .ok_or_else(|| AppError::other("llm_stream_incomplete", json!({})))?;
        let items: Vec<Value> = match response.get("output") {
            Some(Value::Array(items)) => items.clone(),
            None => self.items.into_values().collect(),
            _ => return Err(invalid_response()),
        };
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut call_ids = HashSet::new();
        let mut reasoning = Vec::new();
        let mut messages = Vec::new();
        for item in items {
            match item["type"].as_str() {
                Some("message") => {
                    let message = parse_assistant_message(&item)?;
                    text.push_str(&message.content);
                    messages.push(message);
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
            codex_messages: (!messages.is_empty()).then_some(messages),
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
    use std::sync::{Arc, Mutex};

    #[test]
    fn api_key_client_accepts_base_and_full_responses_urls() {
        for (raw, expected) in [
            (
                "https://api.openai.com/v1",
                "https://api.openai.com/v1/responses",
            ),
            (
                " https://api.openai.com/v1/responses/ ",
                "https://api.openai.com/v1/responses",
            ),
            (
                "https://proxy.example/v1",
                "https://proxy.example/v1/responses",
            ),
        ] {
            let client = OpenAiResponsesClient::new("test-key".into(), raw.into());
            assert_eq!(client.endpoint, expected);
        }
    }

    #[tokio::test]
    async fn protocol_choice_controls_the_endpoint_for_the_same_base_url() {
        let mut server = mockito::Server::new_async().await;
        for (protocol, path, body) in [
            ("openai-completions", "/v1/chat/completions", "data: [DONE]\n\n"),
            ("openai-responses", "/v1/responses", "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n"),
        ] {
            let expected = server.mock("POST", path)
                .with_header("content-type", "text/event-stream")
                .with_body(body)
                .create_async().await;
            let client = super::super::build_client(protocol, "test-key".into(), format!("{}/v1", server.url())).unwrap();
            client.chat(text_request("gpt-6.1-sol"), Arc::new(|_| {})).await.unwrap();
            expected.assert_async().await;
        }
    }

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
    fn assistant_phases_and_boundaries_survive_history_and_redaction() {
        let (sink, _) = sink();
        let mut state = ResponseState::default();
        let mut commentary = message("Checking 192.168.1.1");
        commentary["phase"] = json!("commentary");
        let mut answer = message("Checked 10.0.0.1");
        answer["phase"] = json!("final_answer");
        state
            .event(
                completed(vec![commentary, answer, message("No phase")]),
                &sink,
            )
            .unwrap();
        let response = state.finish(&sink).unwrap();
        // The session stores response text as content and keeps the Codex
        // metadata. Round-trip the same fields through the persisted format.
        let mut stored = serde_json::to_value(response).unwrap();
        stored["role"] = json!("assistant");
        stored["content"] = stored["text"].take();
        let history: ChatMessage =
            serde_json::from_str(&serde_json::to_string(&stored).unwrap()).unwrap();
        let history =
            crate::ai::sanitize::redact_message(&history, &crate::ai::sanitize::default_rules());
        let body = build_request(&ChatRequest {
            system_prompt: "Diagnose the host".into(),
            messages: vec![history],
            tools: vec![],
            model: "account-model".into(),
            max_tokens: 4096,
        });
        assert_eq!(body["input"].as_array().unwrap().len(), 3);
        assert_eq!(body["input"][0]["phase"], "commentary");
        assert_eq!(body["input"][1]["phase"], "final_answer");
        assert!(body["input"][2].get("phase").is_none());
        assert_eq!(body["input"][2]["content"][0]["text"], "No phase");
        assert_eq!(
            body["input"][0]["content"][0]["text"],
            "Checking <REDACTED:ip-192>"
        );
        assert_eq!(
            body["input"][1]["content"][0]["text"],
            "Checked <REDACTED:ip-10>"
        );
        assert!(!body.to_string().contains("192.168.1.1"));
        assert!(!body.to_string().contains("10.0.0.1"));
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
                    codex_messages: None,
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
        assert_eq!(body["input"][2]["content"][0]["text"], "Checking disk");
        assert!(body["input"][2].get("phase").is_none());
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
            "llm_stream_incomplete"
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
                "llm_response_failed"
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
                "llm_invalid_response"
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
            "llm_invalid_response"
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

    fn text_request(model: &str) -> ChatRequest {
        ChatRequest {
            system_prompt: "sys".into(),
            messages: vec![ChatMessage::User {
                content: "hello".into(),
            }],
            tools: vec![],
            model: model.into(),
            max_tokens: 1024,
        }
    }

    #[tokio::test]
    async fn responses_supports_gpt4o_and_the_api_key_model_catalog() {
        let mut server = mockito::Server::new_async().await;
        let expected = server
            .mock("POST", "/v1/responses")
            .match_header("authorization", "Bearer test-key")
            .match_body(mockito::Matcher::Json(json!({
                "model": "gpt-4o", "instructions": "sys", "max_output_tokens": 1024,
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hello"}]}],
                "tools": [], "tool_choice": "auto", "parallel_tool_calls": false,
                "include": ["reasoning.encrypted_content"], "store": false, "stream": true
            })))
            .with_header("content-type", "text/event-stream")
            .with_body(format!(
                "data: {}\n\n",
                json!({"type": "response.completed", "response": {
                    "status": "completed", "output": [{"type": "message", "role": "assistant",
                        "content": [{"type": "output_text", "text": "ok"}]}]
                }})
            ))
            .create_async()
            .await;
        let catalog = server
            .mock("GET", "/v1/models")
            .match_header("authorization", "Bearer test-key")
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"id":"gpt-6.1-sol"},{"id":"gpt-4o"}]}"#)
            .create_async()
            .await;
        let client = OpenAiResponsesClient::new(
            "test-key".into(),
            format!("{}/v1/responses/", server.url()),
        );
        assert_eq!(
            client
                .chat(text_request("gpt-4o"), std::sync::Arc::new(|_| {}))
                .await
                .unwrap()
                .text,
            "ok"
        );
        assert_eq!(
            client
                .list_models()
                .await
                .unwrap()
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-4o", "gpt-6.1-sol"]
        );
        expected.assert_async().await;
        catalog.assert_async().await;
    }

    #[tokio::test]
    async fn incomplete_responses_never_return_executable_tool_calls() {
        let mut server = mockito::Server::new_async().await;
        let expected = server.mock("POST", "/v1/responses")
            .with_header("content-type", "text/event-stream")
            .with_body(format!("data: {}\n\ndata: [DONE]\n\n", json!({
                "type": "response.output_item.done", "output_index": 0, "item": {
                    "type": "function_call", "call_id": "call-1", "name": "run_command", "arguments": "{}"
                }
            })))
            .create_async().await;
        let client =
            OpenAiResponsesClient::new("test-key".into(), format!("{}/v1/responses", server.url()));
        let error = client
            .chat(text_request("gpt-6.1-sol"), std::sync::Arc::new(|_| {}))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "llm_stream_incomplete");
        expected.assert_async().await;
    }

    #[tokio::test]
    async fn responses_tools_and_reasoning_survive_a_second_request() {
        let mut server = mockito::Server::new_async().await;
        let reasoning = json!({
            "type": "reasoning", "id": "rs-1", "encrypted_content": "opaque", "summary": []
        });
        let call = json!({
            "type": "function_call", "call_id": "call-1", "name": "run_command",
            "arguments": "{\"command\":\"pwd\"}"
        });
        let first = server
            .mock("POST", "/v1/responses")
            .match_header("authorization", "Bearer test-key")
            .match_body(mockito::Matcher::Json(json!({
                "model": "gpt-6.1-sol", "instructions": "Use approved tools",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "Check directory"}]}],
                "tools": [{"type": "function", "name": "run_command", "description": "Run a command",
                    "parameters": {"type": "object", "properties": {"command": {"type": "string"}}}, "strict": false}],
                "tool_choice": "auto", "parallel_tool_calls": false,
                "include": ["reasoning.encrypted_content"], "store": false, "stream": true,
                "max_output_tokens": 4096
            })))
            .with_header("content-type", "text/event-stream")
            .with_body(format!("data: {}\n\n", json!({
                "type": "response.completed", "response": {"status": "completed", "output": [reasoning, call],
                    "usage": {"input_tokens": 42, "output_tokens": 7}}
            })))
            .create_async().await;
        let client = super::super::build_client(
            "openai-responses",
            "test-key".into(),
            format!("{}/v1/responses", server.url()),
        )
        .unwrap();
        let mut request = ChatRequest {
            system_prompt: "Use approved tools".into(),
            messages: vec![ChatMessage::User {
                content: "Check directory".into(),
            }],
            tools: vec![super::super::ToolSchema {
                name: "run_command".into(),
                description: "Run a command".into(),
                input_schema: json!({"type": "object", "properties": {"command": {"type": "string"}}}),
            }],
            model: "gpt-6.1-sol".into(),
            max_tokens: 4096,
        };
        let sink: DeltaSink = std::sync::Arc::new(|_| {});
        let response = client.chat(request.clone(), sink.clone()).await.unwrap();
        first.assert_async().await;
        assert_eq!(response.tool_calls[0].id, "call-1");
        assert_eq!(response.tool_calls[0].input, json!({"command": "pwd"}));
        assert_eq!(response.tokens_in, Some(42));
        assert_eq!(response.tokens_out, Some(7));
        request.messages.push(ChatMessage::Assistant {
            content: response.text,
            tool_calls: response.tool_calls,
            reasoning_content: response.reasoning_content,
            codex_reasoning: response.codex_reasoning,
            codex_messages: response.codex_messages,
        });
        request.messages.push(ChatMessage::ToolResult {
            tool_call_id: "call-1".into(),
            content: "/tmp".into(),
            is_error: false,
            pre_redacted: false,
        });
        let second = server
            .mock("POST", "/v1/responses")
            .match_body(mockito::Matcher::PartialJson(json!({"input": [
                {"role": "user", "content": [{"type": "input_text", "text": "Check directory"}]},
                reasoning, call, {"type": "function_call_output", "call_id": "call-1", "output": "/tmp"}
            ]})))
            .with_header("content-type", "text/event-stream")
            .with_body(format!("data: {}\n\ndata: {}\n\n",
                json!({"type": "response.output_text.delta", "delta": "目录"}),
                json!({"type": "response.completed", "response": {"status": "completed", "output": [
                    {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "目录 /tmp"}]}
                ]}})))
            .create_async().await;
        let response = client.chat(request, sink).await.unwrap();
        second.assert_async().await;
        assert_eq!(response.text, "目录 /tmp");
        assert!(response.tool_calls.is_empty());
    }
}
