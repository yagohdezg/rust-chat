use async_stream::stream;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::{ChatChunk, ChatRequest, ChatStream, LlmProvider, ModelInfo, ProviderError};

/// An OpenAI-compatible chat provider (also covers Azure-style `/chat/completions`
/// gateways and local servers like Ollama, vLLM and LM Studio).
pub struct OpenAiProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

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
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
        }
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

fn snippet(body: &str) -> String {
    let trimmed = body.trim();
    let end = trimmed
        .char_indices()
        .nth(200)
        .map(|(i, _)| i)
        .unwrap_or(trimmed.len());
    trimmed[..end].to_string()
}

/// Whether a response body looks like an HTML page rather than JSON/SSE. Web UIs
/// (e.g. LiteLLM's admin app) answer unknown paths with HTML, which is useless
/// as an error message.
fn looks_like_html(body: &str) -> bool {
    body.trim_start().starts_with('<')
}

/// Turn a failed HTTP response into an actionable error: web-UI HTML becomes a
/// hint about the base URL, everything else a truncated status + body.
fn http_error(root: &str, status: u16, body: &str) -> ProviderError {
    if looks_like_html(body) {
        return ProviderError::Other(format!(
            "HTTP {status} from {root} returned an HTML page, not the \
             OpenAI-compatible API. The base URL likely points at a provider web UI; \
             for LiteLLM use the proxy API root (e.g. http://host:4000 or \
             http://host:4000/v1) with a valid key, not the admin UI URL."
        ));
    }
    ProviderError::Http {
        status,
        body: snippet(body),
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn stream(&self, request: ChatRequest) -> std::result::Result<ChatStream, ProviderError> {
        let mut body = json!({
            "model": request.model,
            "messages": request.messages,
            "stream": true,
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

        // Try each candidate API root until one accepts the request.
        let mut last_error = None;
        let mut response = None;
        for root in self.roots() {
            let candidate = self
                .http
                .post(format!("{root}/chat/completions"))
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await;
            match candidate {
                Ok(resp) if resp.status().is_success() => {
                    response = Some(resp);
                    break;
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    last_error = Some(http_error(&root, status, &body));
                }
                Err(err) => last_error = Some(ProviderError::Transport(err)),
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
                                let tool_calls = delta.get("tool_calls").filter(|v| !v.is_null()).cloned();
                                let finish_reason = choice
                                    .get("finish_reason")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned);
                                yield Ok(ChatChunk {
                                    delta: content,
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
            let response = match self
                .http
                .get(format!("{root}/models"))
                .bearer_auth(&self.api_key)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(ProviderError::Transport(err));
                    continue;
                }
            };

            let status = response.status();
            // Read as text first so a non-JSON error page produces a useful
            // message instead of reqwest's opaque "error decoding response".
            let body = match response.text().await {
                Ok(body) => body,
                Err(err) => {
                    last_error = Some(ProviderError::Transport(err));
                    continue;
                }
            };
            if !status.is_success() {
                last_error = Some(http_error(&root, status.as_u16(), &body));
                continue;
            }

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
