//! LLM provider abstraction plus concrete backends.
//!
//! The core contract is [`LlmProvider::stream`], which returns a stream of
//! [`ChatChunk`]s. Tool-call deltas are carried verbatim in each chunk so the
//! agent runtime can assemble them.

pub mod openai;

use std::pin::Pin;

use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use openai::OpenAiProvider;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
            ..Default::default()
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
            ..Default::default()
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
            ..Default::default()
        }
    }
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// OpenAI-style tool definitions (JSON schema).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatChunk {
    /// Incremental text delta (may be empty on tool-call-only chunks).
    #[serde(default)]
    pub delta: String,
    /// Raw tool-call deltas as emitted by the provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("http error {status}: {body}")]
    Http { status: u16, body: String },
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("stream decode error: {0}")]
    Decode(String),
    #[error("provider error: {0}")]
    Other(String),
}

pub type ChatStream =
    Pin<Box<dyn Stream<Item = std::result::Result<ChatChunk, ProviderError>> + Send>>;

#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;

    /// Stream a completion. Providers must not buffer the whole response.
    async fn stream(&self, request: ChatRequest) -> std::result::Result<ChatStream, ProviderError>;
}
