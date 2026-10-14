//! LLM provider abstraction plus concrete backends.
//!
//! The core contract is [`LlmProvider::stream`], which returns a stream of
//! [`ChatChunk`]s. Tool-call deltas are carried verbatim in each chunk so the
//! agent runtime can assemble them.

pub mod failover;
pub mod openai;

use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use failover::FailoverProvider;
pub use openai::OpenAiProvider;

/// An image attached to a message, forwarded to a vision-capable model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePart {
    /// Sniffed MIME type, used to build the `data:` URL.
    pub mime: String,
    /// Base64-encoded bytes, without the data-URL prefix.
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    /// Images to forward alongside `content`. Only vision-capable providers
    /// receive these; a text-only provider is never sent an image part.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImagePart>,
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

/// A model advertised by a provider (OpenAI-compatible `/models` entry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
}

/// Token counts a provider reported for one completion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

impl TokenUsage {
    /// Fold another report into this one.
    pub fn add(&mut self, other: &TokenUsage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
    }

    /// Whether the report carries any tokens at all.
    pub fn is_empty(&self) -> bool {
        self.total_tokens == 0
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatChunk {
    /// Token counts the provider reported for this completion, when it reports
    /// them (OpenAI-compatible servers send them on a final, choices-less chunk
    /// once `stream_options.include_usage` is requested).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// Incremental text delta (may be empty on tool-call-only chunks).
    #[serde(default)]
    pub delta: String,
    /// Incremental reasoning/thinking delta (empty for models that do not emit
    /// one).
    #[serde(default)]
    pub reasoning: String,
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
    Http {
        status: u16,
        body: String,
        /// Delay the server asked for via `Retry-After`, when it sent one.
        retry_after: Option<Duration>,
    },
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("stream decode error: {0}")]
    Decode(String),
    #[error("provider error: {0}")]
    Other(String),
}

impl ProviderError {
    /// Whether another attempt could succeed: a rate limit, a server-side
    /// error, or a transport failure. A rejected request (4xx other than 429)
    /// is not retried.
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::Http { status, .. } => is_retryable_status(*status),
            ProviderError::Transport(_) => true,
            ProviderError::Decode(_) | ProviderError::Other(_) => false,
        }
    }
}

/// Whether a status code is worth retrying: rate limits and server errors.
pub fn is_retryable_status(status: u16) -> bool {
    status == 429 || status >= 500
}

/// The delay a response asks for via `Retry-After`, in whole seconds. HTTP-date
/// values are ignored; the exponential backoff applies instead.
pub fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Bounded retry policy for provider calls that fail with a rate limit, a
/// server error or a transport failure. Attempts double the delay each time,
/// starting at `base_delay_ms` and capped at `max_delay_ms`; a server-supplied
/// `Retry-After` replaces the computed delay when it is longer.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Attempts after the first one. `0` disables retrying.
    pub max_retries: u32,
    /// Delay before the first retry, in milliseconds.
    pub base_delay_ms: u64,
    /// Upper bound for a single delay, in milliseconds.
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay_ms: 250,
            max_delay_ms: 8_000,
        }
    }
}

impl RetryPolicy {
    /// Run `send` until it returns a response or the retry budget is spent.
    /// `send` is called once per attempt, so it must rebuild the request; the
    /// last error is returned when no attempt succeeds.
    pub async fn run<F, Fut>(
        &self,
        mut send: F,
    ) -> std::result::Result<reqwest::Response, ProviderError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = std::result::Result<reqwest::Response, ProviderError>>,
    {
        let mut attempt = 0;
        loop {
            match send().await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    if attempt >= self.max_retries || !err.is_retryable() {
                        return Err(err);
                    }
                    let asked = match &err {
                        ProviderError::Http { retry_after, .. } => *retry_after,
                        _ => None,
                    };
                    tokio::time::sleep(self.delay(attempt, asked)).await;
                }
            }
            attempt += 1;
        }
    }

    /// Delay before retry `attempt` (0-based): `base_delay_ms * 2^attempt`,
    /// capped at `max_delay_ms`. A server-supplied `Retry-After` is honored as
    /// sent when it asks for a longer wait.
    fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        let backoff = Duration::from_millis(
            self.base_delay_ms
                .saturating_mul(1u64 << attempt.min(16))
                .min(self.max_delay_ms),
        );
        match retry_after {
            Some(asked) if asked > backoff => asked,
            _ => backoff,
        }
    }
}

pub type ChatStream =
    Pin<Box<dyn Stream<Item = std::result::Result<ChatChunk, ProviderError>> + Send>>;

#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;

    /// Stream a completion. Providers must not buffer the whole response.
    async fn stream(&self, request: ChatRequest) -> std::result::Result<ChatStream, ProviderError>;

    /// List the models this provider (or its key) can serve. Used to populate
    /// model pickers and to validate a requested model before a completion.
    async fn list_models(&self) -> std::result::Result<Vec<ModelInfo>, ProviderError>;
}
