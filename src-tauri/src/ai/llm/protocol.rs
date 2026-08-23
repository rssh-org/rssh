//! OpenAI Completions 协议客户端。
//!
//! 覆盖所有走 Chat Completions wire 格式的服务端（OpenAI / 智谱 GLM /
//! vLLM / Together / Groq …）。DeepSeek Thinking 是**独立协议文件**
//! （`deepseek.rs`）——它的 reasoning_content 回传语义不属于这里，共享
//! 实现曾把思考链发给不认识它的服务端（跨协议泄漏），已结构性消灭。
//!
//! 参考：https://platform.openai.com/docs/api-reference/chat

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::json;

use super::{
    ChatDelta, ChatMessage, ChatRequest, ChatResponse, DeltaSink, LlmClient, ModelInfo, SseParser,
    ToolCall,
};
use crate::error::{error_chain, AppError, AppResult};

/// Client for the OpenAI Completions protocol. `endpoint` is required —
/// callers pass the provider row's explicit value.
pub struct OpenAiCompletionsClient {
    api_key: String,
    chat_endpoint: String,
    http: reqwest::Client,
}

impl OpenAiCompletionsClient {
    pub fn new(api_key: String, endpoint: String) -> Self {
        Self {
            api_key,
            chat_endpoint: normalize_chat_endpoint(endpoint),
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmClient for OpenAiCompletionsClient {
    async fn chat(&self, req: ChatRequest, sink: DeltaSink) -> AppResult<ChatResponse> {
        chat(&self.http, &self.chat_endpoint, &self.api_key, req, sink).await
    }

    async fn list_models(&self) -> AppResult<Vec<ModelInfo>> {
        let url = models_endpoint_from_chat(&self.chat_endpoint);
        list_models(&self.http, &url, &self.api_key).await
    }
}

/// 归一化成"chat completions URL"。接受两种输入：
///   - 完整 chat URL（`.../v1/chat/completions`）—— 直接用
///   - base URL（`.../v1`）—— 自动拼 `/chat/completions`
fn normalize_chat_endpoint(raw: String) -> String {
    let trimmed = raw.trim().trim_end_matches('/').to_string();
    if trimmed.ends_with("/chat/completions") {
        trimmed
    } else {
        format!("{trimmed}/chat/completions")
    }
}

/// 从 chat endpoint 推 `/models` URL。OpenAI 兼容惯例：把 `/chat/completions` 换成 `/models`。
fn models_endpoint_from_chat(chat_endpoint: &str) -> String {
    if let Some(base) = chat_endpoint.strip_suffix("/chat/completions") {
        format!("{base}/models")
    } else {
        format!("{chat_endpoint}/models")
    }
}

// ─── 请求体序列化 ──────────────────────────────────────────────────

#[derive(Serialize)]
struct OaiReq<'a> {
    model: &'a str,
    max_completion_tokens: u32,
    messages: Vec<OaiMsg>,
    tools: Vec<serde_json::Value>,
    stream: bool,
    stream_options: serde_json::Value,
}

#[derive(Serialize)]
struct OaiMsg {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OaiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct OaiToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: OaiToolCallFn,
}

#[derive(Serialize)]
struct OaiToolCallFn {
    name: String,
    arguments: String,
}

// ─── 主入口：流式 chat ─────────────────────────────────────────────

pub async fn chat(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    req: ChatRequest,
    sink: DeltaSink,
) -> AppResult<ChatResponse> {
    let mut messages: Vec<OaiMsg> = Vec::with_capacity(req.messages.len() + 1);
    messages.push(OaiMsg {
        role: "system",
        content: Some(req.system_prompt.clone()),
        tool_calls: None,
        tool_call_id: None,
    });
    for m in &req.messages {
        match m {
            ChatMessage::User { content } => messages.push(OaiMsg {
                role: "user",
                content: Some(content.clone()),
                tool_calls: None,
                tool_call_id: None,
            }),
            // `reasoning_content`（DeepSeek 思考链）在这里被显式丢弃：本协议
            // 的服务端不认识该字段，透传是脏数据泄漏（还可能 400）。
            ChatMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                let oai_calls: Vec<OaiToolCall> = tool_calls
                    .iter()
                    .map(|tc| OaiToolCall {
                        id: tc.id.clone(),
                        kind: "function",
                        function: OaiToolCallFn {
                            name: tc.name.clone(),
                            arguments: serde_json::to_string(&tc.input).unwrap_or_default(),
                        },
                    })
                    .collect();
                messages.push(OaiMsg {
                    role: "assistant",
                    content: if content.is_empty() {
                        None
                    } else {
                        Some(content.clone())
                    },
                    tool_calls: if oai_calls.is_empty() {
                        None
                    } else {
                        Some(oai_calls)
                    },
                    tool_call_id: None,
                });
            }
            ChatMessage::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                let body = if *is_error {
                    format!("[ERROR] {content}")
                } else {
                    content.clone()
                };
                messages.push(OaiMsg {
                    role: "tool",
                    content: Some(body),
                    tool_calls: None,
                    tool_call_id: Some(tool_call_id.clone()),
                });
            }
        }
    }

    let tools: Vec<serde_json::Value> = req
        .tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            })
        })
        .collect();

    let body = OaiReq {
        model: &req.model,
        max_completion_tokens: req.max_tokens,
        messages,
        tools,
        stream: true,
        stream_options: json!({ "include_usage": true }),
    };

    let resp = http
        .post(endpoint)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::other("llm_request_failed", json!({ "err": error_chain(&e) })))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::other(
            "llm_error_status",
            json!({ "status": status.to_string(), "text": text }),
        ));
    }

    let mut text_out = String::new();
    let mut tool_calls: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
    let mut finish_reason = String::new();
    let mut tokens_in: Option<u32> = None;
    let mut tokens_out: Option<u32> = None;

    let mut parser = SseParser::new();
    let mut stream = resp.bytes_stream();
    'stream: while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| {
            AppError::other("llm_stream_read_failed", json!({ "err": error_chain(&e) }))
        })?;
        for ev_data in parser.feed(&bytes) {
            if ev_data.trim() == "[DONE]" {
                break 'stream;
            }
            let v: serde_json::Value = match serde_json::from_str(&ev_data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(usage) = v.get("usage") {
                tokens_in = usage["prompt_tokens"].as_u64().map(|n| n as u32);
                tokens_out = usage["completion_tokens"].as_u64().map(|n| n as u32);
            }
            if let Some(choice) = v["choices"].get(0) {
                if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                    finish_reason = reason.to_string();
                }
                let delta = &choice["delta"];
                if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                    if !content.is_empty() {
                        text_out.push_str(content);
                        sink(ChatDelta::Text(content.to_string()));
                    }
                }
                if let Some(tcs_arr) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tcs_arr {
                        let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let entry = tool_calls
                            .entry(idx)
                            .or_insert_with(|| (String::new(), String::new(), String::new()));
                        if let Some(id) = tc.get("id").and_then(|s| s.as_str()) {
                            if entry.0.is_empty() && !id.is_empty() {
                                entry.0 = id.to_string();
                            }
                        }
                        if let Some(name) = tc["function"].get("name").and_then(|s| s.as_str()) {
                            if entry.1.is_empty() && !name.is_empty() {
                                entry.1 = name.to_string();
                                sink(ChatDelta::ToolStart {
                                    tool_call_id: entry.0.clone(),
                                    name: entry.1.clone(),
                                });
                            }
                        }
                        if let Some(args) = tc["function"].get("arguments").and_then(|a| a.as_str())
                        {
                            if !args.is_empty() {
                                entry.2.push_str(args);
                                sink(ChatDelta::ToolArgs {
                                    tool_call_id: entry.0.clone(),
                                    partial: args.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    let tcs: Vec<ToolCall> = tool_calls
        .into_values()
        .map(|(id, name, args)| ToolCall {
            id,
            name,
            input: serde_json::from_str(&args).unwrap_or(serde_json::Value::Null),
        })
        .collect();

    Ok(ChatResponse {
        text: text_out,
        tool_calls: tcs,
        stop_reason: finish_reason,
        tokens_in,
        tokens_out,
        // The OpenAI Completions protocol carries no thinking chain.
        reasoning_content: None,
    })
}

// ─── /models 列表 ──────────────────────────────────────────────────

/// GET {endpoint}，Bearer 认证，解析 `{ "data": [{ "id": ... }, ...] }`。
/// 服务端不开放该接口（如智谱）时返回错误 —— UI 层下拉留空即可。
pub async fn list_models(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> AppResult<Vec<ModelInfo>> {
    let resp = http
        .get(endpoint)
        .bearer_auth(api_key)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|e| AppError::other("llm_request_failed", json!({ "err": error_chain(&e) })))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::other(
            "llm_error_status",
            json!({ "status": status.to_string(), "text": text }),
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AppError::other("llm_decode_failed", json!({ "err": error_chain(&e) })))?;
    let data = v["data"].as_array().cloned().unwrap_or_default();
    let mut models: Vec<ModelInfo> = data
        .into_iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let display_name = m
                .get("display_name")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            Some(ModelInfo { id, display_name })
        })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A history Assistant message carrying a DeepSeek-style thinking chain
    /// must serialize WITHOUT the reasoning_content field — the regression
    /// test for the cross-protocol leak (it used to ride the shared impl).
    #[test]
    fn assistant_reasoning_content_is_never_serialized() {
        let msg = ChatMessage::Assistant {
            content: "answer".into(),
            tool_calls: vec![],
            reasoning_content: Some("secret chain of thought".into()),
        };
        let oai = match &msg {
            ChatMessage::Assistant {
                content,
                tool_calls,
                ..
            } => OaiMsg {
                role: "assistant",
                content: if content.is_empty() {
                    None
                } else {
                    Some(content.clone())
                },
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(vec![])
                },
                tool_call_id: None,
            },
            _ => unreachable!(),
        };
        let s = serde_json::to_string(&oai).unwrap();
        assert!(!s.contains("reasoning_content"));
        assert!(!s.contains("secret chain"));
    }

    #[test]
    fn normalizes_base_and_full_chat_urls() {
        assert_eq!(
            normalize_chat_endpoint("https://api.openai.com/v1".into()),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            normalize_chat_endpoint("https://api.openai.com/v1/chat/completions/".into()),
            "https://api.openai.com/v1/chat/completions"
        );
    }
}
