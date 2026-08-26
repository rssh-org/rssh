//! DeepSeek Thinking 协议（独立实现，与 `protocol.rs` 的 OpenAI Completions
//! 完全拆分）。
//!
//! wire 层接近 Chat Completions，但有一条 DeepSeek 特有硬约束：带 `tools`
//! 的请求在后续所有轮次必须把 assistant 消息的 `reasoning_content`（思考链）
//! 原样回传，否则 400（官方 Thinking Mode 文档：
//! https://api-docs.deepseek.com/guides/thinking_mode/ ）。该语义只存在于
//! 本文件 —— 其他协议遇到思考链一律丢弃（见 protocol.rs / anthropic.rs）。
//!
//! 模型：deepseek-chat / deepseek-reasoner / deepseek-v3.x（以 /models 拉取为准）。
//! 文档：https://api-docs.deepseek.com/

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

pub struct DeepSeekClient {
    api_key: String,
    chat_endpoint: String,
    http: reqwest::Client,
}

impl DeepSeekClient {
    /// `endpoint` 必填（provider 行的显式值）。接受 base URL（`.../v1` 或
    /// 裸域名）或完整 chat URL，统一归一化成 chat completions URL。
    pub fn new(api_key: String, endpoint: String) -> Self {
        let trimmed = endpoint.trim().trim_end_matches('/').to_string();
        let chat_endpoint = if trimmed.ends_with("/chat/completions") {
            trimmed
        } else {
            format!("{trimmed}/chat/completions")
        };
        Self {
            api_key,
            chat_endpoint,
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmClient for DeepSeekClient {
    async fn chat(&self, req: ChatRequest, sink: DeltaSink) -> AppResult<ChatResponse> {
        chat(&self.http, &self.chat_endpoint, &self.api_key, req, sink).await
    }

    async fn list_models(&self) -> AppResult<Vec<ModelInfo>> {
        let url = match self.chat_endpoint.strip_suffix("/chat/completions") {
            Some(base) => format!("{base}/models"),
            None => format!("{}/models", self.chat_endpoint),
        };
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.api_key)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                AppError::other("llm_request_failed", json!({ "err": error_chain(&e) }))
            })?;
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
        let mut models: Vec<ModelInfo> = v["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| {
                let id = m.get("id")?.as_str()?.to_string();
                Some(ModelInfo {
                    id,
                    display_name: None,
                })
            })
            .collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }
}

// ─── 请求体序列化 ──────────────────────────────────────────────────

#[derive(Serialize)]
struct DsReq<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<DsMsg>,
    tools: Vec<serde_json::Value>,
    stream: bool,
    stream_options: serde_json::Value,
}

#[derive(Serialize)]
struct DsMsg {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<DsToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    /// DeepSeek Thinking 特有：带 tools 的多轮必须把上轮思考链原样塞回，
    /// 否则 400。仅 assistant 消息可能携带。
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

#[derive(Serialize)]
struct DsToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: DsToolCallFn,
}

#[derive(Serialize)]
struct DsToolCallFn {
    name: String,
    arguments: String,
}

/// Map the shared ChatMessage history onto the DeepSeek wire shape. Extracted
/// from `chat()` so the reasoning_content echo is directly testable.
fn build_messages(system: &str, history: &[ChatMessage]) -> Vec<DsMsg> {
    let mut messages: Vec<DsMsg> = Vec::with_capacity(history.len() + 1);
    messages.push(DsMsg {
        role: "system",
        content: Some(system.to_string()),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    });
    for m in history {
        match m {
            ChatMessage::User { content } => messages.push(DsMsg {
                role: "user",
                content: Some(content.clone()),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
            }),
            ChatMessage::Assistant {
                content,
                tool_calls,
                reasoning_content,
            } => {
                let ds_calls: Vec<DsToolCall> = tool_calls
                    .iter()
                    .map(|tc| DsToolCall {
                        id: tc.id.clone(),
                        kind: "function",
                        function: DsToolCallFn {
                            name: tc.name.clone(),
                            arguments: serde_json::to_string(&tc.input).unwrap_or_default(),
                        },
                    })
                    .collect();
                messages.push(DsMsg {
                    role: "assistant",
                    content: if content.is_empty() {
                        None
                    } else {
                        Some(content.clone())
                    },
                    tool_calls: if ds_calls.is_empty() {
                        None
                    } else {
                        Some(ds_calls)
                    },
                    tool_call_id: None,
                    // 思考链回传 —— 本协议存在的核心理由
                    reasoning_content: reasoning_content.clone(),
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
                messages.push(DsMsg {
                    role: "tool",
                    content: Some(body),
                    tool_calls: None,
                    tool_call_id: Some(tool_call_id.clone()),
                    reasoning_content: None,
                });
            }
        }
    }
    messages
}

// ─── 主入口：流式 chat ─────────────────────────────────────────────

async fn chat(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    req: ChatRequest,
    sink: DeltaSink,
) -> AppResult<ChatResponse> {
    let messages = build_messages(&req.system_prompt, &req.messages);

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

    let body = DsReq {
        model: &req.model,
        // DeepSeek 不认 max_completion_tokens，用 max_tokens。
        max_tokens: req.max_tokens,
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
    let mut reasoning_out = String::new();
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
                // 思考链累积：不往 sink 推（不渲染 UI），但必须还回去
                if let Some(rc) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
                    if !rc.is_empty() {
                        reasoning_out.push_str(rc);
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
        reasoning_content: if reasoning_out.is_empty() {
            None
        } else {
            Some(reasoning_out)
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DeepSeek must serialize the thinking chain BACK on assistant messages —
    /// the wire contract that keeps tool-calling multi-turn conversations from
    /// 400ing ("reasoning_content must be passed back"). Runs the production
    /// mapping (`build_messages`), not a hand-built struct.
    #[test]
    fn assistant_reasoning_content_is_echoed() {
        let history = vec![ChatMessage::Assistant {
            content: "answer".into(),
            tool_calls: vec![],
            reasoning_content: Some("chain".into()),
        }];
        let s = serde_json::to_string(&build_messages("sys", &history)).unwrap();
        assert!(s.contains("\"reasoning_content\":\"chain\""));
    }

    #[test]
    fn normalizes_endpoint() {
        let c = DeepSeekClient::new("k".into(), "https://api.deepseek.com/v1".into());
        assert_eq!(
            c.chat_endpoint,
            "https://api.deepseek.com/v1/chat/completions"
        );
        let c = DeepSeekClient::new(
            "k".into(),
            "https://api.deepseek.com/v1/chat/completions".into(),
        );
        assert_eq!(
            c.chat_endpoint,
            "https://api.deepseek.com/v1/chat/completions"
        );
    }
}
