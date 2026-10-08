use async_stream::stream;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::{ChatChunk, ChatRequest, ChatStream, LlmProvider, ProviderError};

/// An OpenAI-compatible chat provider (also covers Azure-style `/chat/completions`
/// gateways and local servers like Ollama, vLLM and LM Studio).
pub struct OpenAiProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
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

        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Http { status, body });
        }

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
}
