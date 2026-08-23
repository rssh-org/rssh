//! BYOK LLM 客户端（流式）。
//!
//! 决议 #5：手写 reqwest + 自解析 SSE，零额外 SDK 依赖。
//!
//! 三种协议类型，一协议一文件，互不共享请求/解析逻辑：
//! - `deepseek.rs`     —— DeepSeek Thinking（含 reasoning_content 回传）
//! - `protocol.rs`     —— OpenAI Completions（OpenAI / GLM / vLLM / Groq …）
//! - `anthropic.rs`    —— Anthropic Messages
//!
//! DeepSeek 与 OpenAI 协议曾共用一套实现，导致 DeepSeek 特有的思考链
//! 回传逻辑长在共享层：换协议 resume 对话时会把 reasoning_content 发给
//! 不认识它的服务端。拆分后思考链语义只存在于 deepseek.rs —— 结构上
//! 消灭跨协议泄漏。
//!
//! `build_client` 按 provider 行的 protocol 字段三分发；endpoint 恒为
//! 用户配置的显式值（必填），没有编译期默认端点。

pub mod anthropic;
pub mod deepseek;
mod protocol;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AppResult;

pub use protocol::OpenAiCompletionsClient;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum ChatMessage {
    User {
        content: String,
    },
    Assistant {
        content: String,
        tool_calls: Vec<ToolCall>,
        /// 部分模型（如 DeepSeek `deepseek-reasoner`）会输出"思考链"。
        /// 这些厂商要求多轮对话时把 reasoning 原样塞回去，否则 400。
        /// 其他厂商：传 None，序列化时不会出现该字段，零影响。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
        /// Internal flag: content was already redacted at insertion site.
        /// `redact_message` skips a second pass — important for structured
        /// JSON payloads (file_ops) where re-redacting hex hashes inside
        /// `package-lock.json` / git oid would corrupt the LLM's view of
        /// the file and trigger downstream `count_mismatch` loops.
        ///
        /// Skipped during serialization so neither LLM provider sees it.
        #[serde(skip)]
        pre_redacted: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub system_prompt: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSchema>,
    pub model: String,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: String,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    /// 思考链（DeepSeek reasoner 之类）。原样传回 history 给下一轮。
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
}

/// 流式增量回调。Text 用于 UI 实时渲染；其余仅供调试 / 暂不消费。
#[derive(Debug, Clone)]
pub enum ChatDelta {
    Text(String),
    ToolStart {
        tool_call_id: String,
        name: String,
    },
    ToolArgs {
        tool_call_id: String,
        partial: String,
    },
}

pub type DeltaSink = Arc<dyn Fn(ChatDelta) + Send + Sync>;

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(&self, req: ChatRequest, sink: DeltaSink) -> AppResult<ChatResponse>;
    async fn list_models(&self) -> AppResult<Vec<ModelInfo>>;
}

/// The closed set of protocols a provider row may declare. `parse` is the
/// single source of truth — `build_client` matches the enum (exhaustive, so a
/// new variant without a client arm fails to compile) and `protocol_valid`
/// derives from it. Wire/DB values stay strings; the enum is internal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    DeepSeekThinking,
    OpenAiCompletions,
    AnthropicMessages,
}

impl Protocol {
    pub fn parse(s: &str) -> Option<Protocol> {
        match s {
            "deepseek-thinking" => Some(Protocol::DeepSeekThinking),
            "openai-completions" => Some(Protocol::OpenAiCompletions),
            "anthropic-messages" => Some(Protocol::AnthropicMessages),
            _ => None,
        }
    }
}

/// Validation derived from `Protocol::parse` — can never drift from dispatch.
pub fn protocol_valid(p: &str) -> bool {
    Protocol::parse(p).is_some()
}

/// Build the client for a provider row's protocol. `endpoint` is the row's
/// explicit (required) endpoint — no compile-time vendor defaults anywhere.
pub fn build_client(
    protocol: &str,
    api_key: String,
    endpoint: String,
) -> AppResult<Box<dyn LlmClient>> {
    match Protocol::parse(protocol) {
        Some(Protocol::DeepSeekThinking) => {
            Ok(Box::new(deepseek::DeepSeekClient::new(api_key, endpoint)))
        }
        Some(Protocol::OpenAiCompletions) => {
            Ok(Box::new(OpenAiCompletionsClient::new(api_key, endpoint)))
        }
        Some(Protocol::AnthropicMessages) => {
            Ok(Box::new(anthropic::AnthropicClient::new(api_key, endpoint)))
        }
        None => Err(crate::error::AppError::config(
            "llm_unknown_protocol",
            serde_json::json!({ "protocol": protocol }),
        )),
    }
}

// ─── SSE 解析公共工具 ────────────────────────────────────────────

/// 增量 SSE 解析器：feed 接收任意 byte chunk，返回完整事件的 data 字符串列表。
pub(crate) struct SseParser {
    /// Decoded-but-not-yet-terminated event text (split on `\n\n`).
    buf: String,
    /// Bytes of an incomplete trailing UTF-8 char from the previous chunk.
    /// `reqwest` splits the stream at arbitrary byte boundaries, so a multibyte
    /// char (CJK / emoji) can straddle two chunks; we hold the partial tail here
    /// and prepend it to the next chunk instead of decoding each chunk alone
    /// (which produced replacement chars on both sides — silent corruption that
    /// still parsed as JSON and got persisted).
    pending: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            pending: Vec::new(),
        }
    }

    /// 喂入新字节，返回若干完整事件的 data 行（多行 data: 已合并）。
    /// 接收原始字节并自己处理 UTF-8 边界 —— 调用方不再各自 from_utf8_lossy。
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.decode_into_buf(chunk);
        let mut events = Vec::new();
        loop {
            let sep_idx = self.buf.find("\n\n").or_else(|| self.buf.find("\r\n\r\n"));
            let Some(idx) = sep_idx else { break };
            let sep_len = if self.buf[idx..].starts_with("\r\n\r\n") {
                4
            } else {
                2
            };
            let event_text = self.buf[..idx].to_string();
            self.buf = self.buf[idx + sep_len..].to_string();

            let mut data_lines: Vec<&str> = Vec::new();
            for line in event_text.lines() {
                let line = line.trim_end_matches('\r');
                if let Some(d) = line.strip_prefix("data:") {
                    data_lines.push(d.strip_prefix(' ').unwrap_or(d));
                }
            }
            if !data_lines.is_empty() {
                events.push(data_lines.join("\n"));
            }
        }
        events
    }

    /// Append `chunk` to `buf`, decoding UTF-8 across chunk boundaries. The
    /// maximal valid prefix is decoded; an incomplete trailing char is stashed
    /// in `pending` for the next call; genuinely invalid bytes become U+FFFD
    /// (the same lossy behavior as before, minus the splitting of real chars at
    /// chunk seams).
    fn decode_into_buf(&mut self, chunk: &[u8]) {
        let mut bytes = std::mem::take(&mut self.pending);
        bytes.extend_from_slice(chunk);

        let mut rest: &[u8] = &bytes;
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    self.buf.push_str(s);
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    // `rest[..valid]` is valid UTF-8 by definition of valid_up_to.
                    self.buf
                        .push_str(std::str::from_utf8(&rest[..valid]).unwrap_or(""));
                    match e.error_len() {
                        // Unexpected end: a valid prefix of a multibyte char sits
                        // at the tail. Hold it for the next chunk.
                        None => {
                            self.pending.extend_from_slice(&rest[valid..]);
                            break;
                        }
                        // n genuinely invalid bytes: emit one replacement char,
                        // skip past them, keep decoding the remainder.
                        Some(n) => {
                            self.buf.push('\u{FFFD}');
                            rest = &rest[valid + n..];
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod sse_tests {
    use super::SseParser;

    #[test]
    fn reassembles_multibyte_char_split_across_chunks() {
        // "中" = E4 B8 AD. Cut right after E4 so each half is invalid UTF-8 on
        // its own — the exact shape reqwest produces mid-stream. Per-chunk lossy
        // decoding (the old bug) yielded U+FFFD on both sides; the parser must
        // instead carry the partial char across feeds.
        let mut p = SseParser::new();
        let event = "data: hi 中\n\n".as_bytes();
        let cut = event.iter().position(|&b| b == 0xE4).unwrap() + 1;
        assert!(p.feed(&event[..cut]).is_empty());
        assert_eq!(p.feed(&event[cut..]), vec!["hi 中".to_string()]);
    }

    #[test]
    fn emits_replacement_for_genuinely_invalid_bytes() {
        // A lone 0xFF is never valid and never a multibyte prefix: it must become
        // U+FFFD (matching lossy) and not be hoarded as "pending" forever, which
        // would stall the stream.
        let mut p = SseParser::new();
        let mut bytes = b"data: ".to_vec();
        bytes.push(0xFF);
        bytes.extend_from_slice(b"x\n\n");
        assert_eq!(p.feed(&bytes), vec!["\u{FFFD}x".to_string()]);
    }

    #[test]
    fn splits_multiple_events_in_one_chunk() {
        // Sanity: event framing still works on the new byte path.
        let mut p = SseParser::new();
        let events = p.feed(b"data: a\n\ndata: b\n\n");
        assert_eq!(events, vec!["a".to_string(), "b".to_string()]);
    }
}
