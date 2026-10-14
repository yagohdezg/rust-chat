use std::time::Duration;

use async_stream::stream;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::{
    error_for_response, looks_like_html, retry_after, snippet, ChatChunk, ChatMessage, ChatRequest,
    ChatStream, LlmProvider, ModelInfo, ProviderError, RetryPolicy, TokenUsage,
};

/// An OpenAI-compatible chat provider (also covers Azure-style `/chat/completions`
/// gateways and local servers like Ollama, vLLM and LM Studio).
pub struct OpenAiProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    retry: RetryPolicy,
}

/// How long one connect attempt may take before it counts as a transport
/// failure. Without a bound an unreachable provider hangs the turn instead of
/// retrying or failing over.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

impl OpenAiProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        // Tolerate keys pasted with surrounding whitespace or a `Bearer ` prefix,
        // both of which would otherwise make the auth header invalid.
        let api_key = api_key.into();
        let api_key = api_key.trim();
        let api_key = api_key
            .strip_prefix("Bearer ")
            .or_else(|| api_key.strip_prefix("bearer "))
            .unwrap_or(api_key)
            .trim()
            .to_string();
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            retry: RetryPolicy::default(),
        }
    }

    /// Retry rate-limited and failed requests under `policy` instead of the
    /// default policy.
    pub fn with_retries(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }

    /// Candidate API roots to try, in order. Many gateways (LiteLLM, Ollama,
    /// LM Studio) serve the OpenAI API under `/v1`, but users often paste just
    /// the host root, so fall back to `<base>/v1` when it is missing.
    fn roots(&self) -> Vec<String> {
        let mut roots = vec![self.base_url.clone()];
        if !self.base_url.ends_with("/v1") {
            roots.push(format!("{}/v1", self.base_url));
        }
        roots
    }
}

/// Render one message in the OpenAI wire format. A message carrying images
/// uses the multi-part content array (`text` + `image_url` data URLs); every
/// other message keeps the plain string content, which is what text-only
/// gateways accept.
fn message_json(message: &ChatMessage) -> Value {
    let content = if message.images.is_empty() {
        json!(message.content)
    } else {
        let mut parts = vec![json!({"type": "text", "text": message.content})];
        for image in &message.images {
            parts.push(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", image.mime, image.data) },
            }));
        }
        json!(parts)
    };
    let mut out = json!({ "role": message.role, "content": content });
    if let Some(name) = &message.name {
        out["name"] = json!(name);
    }
    if let Some(tool_calls) = &message.tool_calls {
        out["tool_calls"] = tool_calls.clone();
    }
    if let Some(tool_call_id) = &message.tool_call_id {
        out["tool_call_id"] = json!(tool_call_id);
    }
    out
}

/// Turn a failed HTTP response into an actionable error: web-UI HTML becomes a
/// hint about the base URL, everything else a truncated status + body.
fn http_error(root: &str, status: u16, body: &str, retry_after: Option<Duration>) -> ProviderError {
    if looks_like_html(body) {
        return ProviderError::Other(format!(
            "HTTP {status} from {root} returned an HTML page, not the \
             OpenAI-compatible API. The base URL likely points at a provider web UI; \
             for LiteLLM use the proxy API root (e.g. http://host:4000 or \
             http://host:4000/v1) with a valid key, not the admin UI URL."
        ));
    }
    error_for_response(status, body, retry_after)
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn stream(&self, request: ChatRequest) -> std::result::Result<ChatStream, ProviderError> {
        let messages: Vec<Value> = request.messages.iter().map(message_json).collect();
        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(m) = request.max_tokens {
            body["max_tokens"] = json!(m);
        }
        if let Some(tools) = request.tools {
            body["tools"] = json!(tools);
        }

        // Try each candidate API root until one accepts the request, retrying a
        // rate limit or server error within a root before moving on.
        let mut last_error = None;
        let mut response = None;
        for root in self.roots() {
            let send = || {
                let root = root.clone();
                let http = self.http.clone();
                let key = self.api_key.clone();
                let body = body.clone();
                async move {
                    let response = http
                        .post(format!("{root}/chat/completions"))
                        .bearer_auth(&key)
                        .json(&body)
                        .send()
                        .await
                        .map_err(ProviderError::Transport)?;
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response);
                    }
                    let asked = retry_after(&response);
                    let body = response.text().await.unwrap_or_default();
                    Err(http_error(&root, status.as_u16(), &body, asked))
                }
            };
            match self.retry.run(send).await {
                Ok(resp) => {
                    response = Some(resp);
                    break;
                }
                Err(err) => last_error = Some(err),
            }
        }
        let response = response.ok_or_else(|| {
            last_error.unwrap_or_else(|| ProviderError::Other("no API root reachable".into()))
        })?;

        let mut events = response.bytes_stream().eventsource();

        let stream = stream! {
            while let Some(event) = events.next().await {
                match event {
                    Ok(event) => {
                        if event.data.trim() == "[DONE]" {
                            yield Ok(ChatChunk { done: true, ..Default::default() });
                            break;
                        }
                        match serde_json::from_str::<Value>(&event.data) {
                            Ok(json) => {
                                let choice = &json["choices"][0];
                                let delta = &choice["delta"];
                                let content = delta["content"].as_str().unwrap_or("").to_string();
                                let reasoning = delta
                                    .get("reasoning_content")
                                    .and_then(Value::as_str)
                                    .or_else(|| delta.get("reasoning").and_then(Value::as_str))
                                    .unwrap_or("")
                                    .to_string();
                                let tool_calls = delta.get("tool_calls").filter(|v| !v.is_null()).cloned();
                                let finish_reason = choice
                                    .get("finish_reason")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned);
                                let usage = json
                                    .get("usage")
                                    .filter(|usage| !usage.is_null())
                                    .and_then(|usage| {
                                        serde_json::from_value::<TokenUsage>(usage.clone()).ok()
                                    });
                                yield Ok(ChatChunk {
                                    usage,
                                    delta: content,
                                    reasoning,
                                    tool_calls,
                                    finish_reason,
                                    done: false,
                                });
                            }
                            Err(err) => {
                                yield Err(ProviderError::Decode(err.to_string()));
                            }
                        }
                    }
                    Err(err) => {
                        yield Err(ProviderError::Decode(err.to_string()));
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn list_models(&self) -> std::result::Result<Vec<ModelInfo>, ProviderError> {
        let mut last_error = None;
        for root in self.roots() {
            let send = || {
                let root = root.clone();
                let http = self.http.clone();
                let key = self.api_key.clone();
                async move {
                    let response = http
                        .get(format!("{root}/models"))
                        .bearer_auth(&key)
                        .send()
                        .await
                        .map_err(ProviderError::Transport)?;
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response);
                    }
                    let asked = retry_after(&response);
                    let body = response.text().await.unwrap_or_default();
                    Err(http_error(&root, status.as_u16(), &body, asked))
                }
            };
            let response = match self.retry.run(send).await {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };

            // Read as text first so a non-JSON error page produces a useful
            // message instead of reqwest's opaque "error decoding response".
            let body = match response.text().await {
                Ok(body) => body,
                Err(err) => {
                    last_error = Some(ProviderError::Transport(err));
                    continue;
                }
            };

            let json: Value = match serde_json::from_str(&body) {
                Ok(json) => json,
                Err(err) => {
                    let hint = if looks_like_html(&body) {
                        " (got an HTML page, not JSON — the base URL points at a web UI, \
                         not the OpenAI-compatible API; for LiteLLM use the proxy root, \
                         e.g. http://host:4000 or http://host:4000/v1)"
                    } else {
                        ""
                    };
                    last_error = Some(ProviderError::Other(format!(
                        "GET {root}/models returned non-JSON{hint}: {err}; body: {}",
                        snippet(&body)
                    )));
                    continue;
                }
            };

            let models: Vec<ModelInfo> = json
                .get("data")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| {
                            let id = entry.get("id").and_then(Value::as_str)?;
                            Some(ModelInfo {
                                id: id.to_string(),
                                owned_by: entry
                                    .get("owned_by")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            return Ok(models);
        }

        Err(last_error.unwrap_or_else(|| ProviderError::Other("no API root reachable".into())))
    }
}
