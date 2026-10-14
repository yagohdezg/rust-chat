//! Anthropic-native provider (the Messages API).
//!
//! Anthropic's wire format differs from the OpenAI one the rest of the crate
//! speaks, so this module translates both ways: the conversation is rendered as
//! Anthropic content blocks (text, image, tool_use, tool_result) with the system
//! prompt lifted to a top-level field, and the event stream is decoded back into
//! the OpenAI-shaped [`ChatChunk`] deltas the agent runtime assembles.

use std::collections::BTreeMap;
use std::time::Duration;

use async_stream::stream;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::{
    error_for_response, retry_after, ChatChunk, ChatMessage, ChatRequest, ChatStream, LlmProvider,
    ModelInfo, ProviderError, RetryPolicy, TokenUsage,
};

/// A provider speaking Anthropic's native Messages API (`POST /v1/messages`).
pub struct AnthropicProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    retry: RetryPolicy,
}

/// How long one connect attempt may take before it counts as a transport
/// failure. Without a bound an unreachable provider hangs the turn instead of
/// retrying or failing over.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The API version Anthropic requires on every request.
const API_VERSION: &str = "2023-06-01";

/// The Messages API has no default for `max_tokens`; use this when the caller
/// has no preference.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Anthropic's own endpoint, used when a provider row carries no base URL.
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

impl AnthropicProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let base_url = base_url.trim().trim_end_matches('/');
        let base_url = if base_url.is_empty() {
            DEFAULT_BASE_URL
        } else {
            base_url
        };
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base_url: base_url.to_string(),
            api_key: api_key.into().trim().to_string(),
            retry: RetryPolicy::default(),
        }
    }

    /// Retry rate-limited and failed requests under `policy` instead of the
    /// default policy.
    pub fn with_retries(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }

    /// Candidate API roots to try, in order. The Messages API lives under
    /// `/v1`, but users often paste just the host root, so fall back to
    /// `<base>/v1` when it is missing.
    fn roots(&self) -> Vec<String> {
        let mut roots = vec![self.base_url.clone()];
        if !self.base_url.ends_with("/v1") {
            roots.push(format!("{}/v1", self.base_url));
        }
        roots
    }
}

/// One content block of a rendered message.
fn text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn image_block(image: &crate::ImagePart) -> Value {
    json!({
        "type": "image",
        "source": { "type": "base64", "media_type": image.mime, "data": image.data },
    })
}

/// The `input` of a `tool_use` block: the accumulated argument JSON when it
/// parses to an object, an empty input otherwise.
fn tool_input(arguments: &str) -> Value {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// Append `blocks` to `out` as a message of `role`, merging into the previous
/// message when it has the same role (Anthropic expects alternating turns, and
/// a tool result must ride in a user turn). A message with no blocks is
/// dropped: Anthropic rejects empty content.
fn push_message(out: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last["role"].as_str() == Some(role) {
            if let Some(content) = last["content"].as_array_mut() {
                content.extend(blocks);
                return;
            }
        }
    }
    out.push(json!({ "role": role, "content": blocks }));
}

/// Render the conversation as Anthropic's `messages` array, returning the
/// system prompt (lifted out of the message list) alongside it.
fn messages_json(messages: &[ChatMessage]) -> (Option<String>, Vec<Value>) {
    let mut system = Vec::new();
    let mut out = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "system" => {
                if !message.content.is_empty() {
                    system.push(message.content.clone());
                }
            }
            "tool" => {
                let id = message.tool_call_id.clone().unwrap_or_default();
                push_message(
                    &mut out,
                    "user",
                    vec![json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": message.content,
                    })],
                );
            }
            "assistant" => {
                let mut blocks = Vec::new();
                if !message.content.is_empty() {
                    blocks.push(text_block(&message.content));
                }
                if let Some(calls) = message.tool_calls.as_ref().and_then(Value::as_array) {
                    for call in calls {
                        let name = call
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let arguments = call
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call.get("id").and_then(Value::as_str).unwrap_or_default(),
                            "name": name,
                            "input": tool_input(arguments),
                        }));
                    }
                }
                push_message(&mut out, "assistant", blocks);
            }
            _ => {
                let mut blocks = Vec::new();
                if !message.content.is_empty() {
                    blocks.push(text_block(&message.content));
                }
                blocks.extend(message.images.iter().map(image_block));
                push_message(&mut out, "user", blocks);
            }
        }
    }
    let system = if system.is_empty() {
        None
    } else {
        Some(system.join("\n\n"))
    };
    (system, out)
}

/// Translate OpenAI-style tool definitions into Anthropic's `tools` entries.
fn tools_json(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|tool| {
            let function = tool.get("function").unwrap_or(tool);
            let name = function.get("name").and_then(Value::as_str)?;
            let mut out = json!({ "name": name });
            if let Some(description) = function.get("description").and_then(Value::as_str) {
                out["description"] = json!(description);
            }
            out["input_schema"] = function
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object" }));
            Some(out)
        })
        .collect()
}

/// Map Anthropic's stop reason onto the `finish_reason` vocabulary the runtime
/// already understands.
fn finish_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("tool_use") => "tool_calls",
        Some("max_tokens") => "length",
        Some("refusal") => "content_filter",
        _ => "stop",
    }
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn stream(&self, request: ChatRequest) -> std::result::Result<ChatStream, ProviderError> {
        let (system, messages) = messages_json(&request.messages);
        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "messages": messages,
            "stream": true,
        });
        if let Some(system) = system {
            body["system"] = json!(system);
        }
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(tools) = request.tools.as_deref() {
            let tools = tools_json(tools);
            if !tools.is_empty() {
                body["tools"] = json!(tools);
            }
        }

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
                        .post(format!("{root}/messages"))
                        .header("x-api-key", key)
                        .header("anthropic-version", API_VERSION)
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
                    Err(error_for_response(status.as_u16(), &body, asked))
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
            // `input_tokens` arrives on `message_start` and `output_tokens` on
            // `message_delta`, so a usage report can only be assembled at the
            // end of the message.
            let mut input_tokens = 0u64;
            // Tool-use blocks carry the content-block index they were opened
            // at, but the runtime indexes tool calls densely; keep the mapping.
            let mut tool_index: BTreeMap<u64, usize> = BTreeMap::new();
            while let Some(event) = events.next().await {
                let event = match event {
                    Ok(event) => event,
                    Err(err) => {
                        yield Err(ProviderError::Decode(err.to_string()));
                        break;
                    }
                };
                let data: Value = match serde_json::from_str(&event.data) {
                    Ok(data) => data,
                    Err(_) => continue,
                };
                match data["type"].as_str().unwrap_or_default() {
                    "message_start" => {
                        input_tokens = data
                            .pointer("/message/usage/input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                    }
                    "content_block_start" => {
                        let block = &data["content_block"];
                        if block["type"].as_str() == Some("tool_use") {
                            let index = data["index"].as_u64().unwrap_or(0);
                            let ordinal = tool_index.len();
                            tool_index.insert(index, ordinal);
                            yield Ok(ChatChunk {
                                tool_calls: Some(json!([{
                                    "index": ordinal,
                                    "id": block["id"],
                                    "type": "function",
                                    "function": { "name": block["name"], "arguments": "" },
                                }])),
                                ..Default::default()
                            });
                        }
                    }
                    "content_block_delta" => {
                        let delta = &data["delta"];
                        match delta["type"].as_str().unwrap_or_default() {
                            "text_delta" => {
                                if let Some(text) = delta["text"].as_str() {
                                    if !text.is_empty() {
                                        yield Ok(ChatChunk {
                                            delta: text.to_string(),
                                            ..Default::default()
                                        });
                                    }
                                }
                            }
                            "thinking_delta" => {
                                if let Some(thinking) = delta["thinking"].as_str() {
                                    if !thinking.is_empty() {
                                        yield Ok(ChatChunk {
                                            reasoning: thinking.to_string(),
                                            ..Default::default()
                                        });
                                    }
                                }
                            }
                            "input_json_delta" => {
                                let index = data["index"].as_u64().unwrap_or(0);
                                if let (Some(ordinal), Some(partial)) = (
                                    tool_index.get(&index),
                                    delta["partial_json"].as_str(),
                                ) {
                                    if !partial.is_empty() {
                                        yield Ok(ChatChunk {
                                            tool_calls: Some(json!([{
                                                "index": ordinal,
                                                "function": { "arguments": partial },
                                            }])),
                                            ..Default::default()
                                        });
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    "message_delta" => {
                        let completion_tokens = data
                            .pointer("/usage/output_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        yield Ok(ChatChunk {
                            usage: Some(TokenUsage {
                                prompt_tokens: input_tokens,
                                completion_tokens,
                                total_tokens: input_tokens + completion_tokens,
                            }),
                            finish_reason: Some(
                                finish_reason(
                                    data.pointer("/delta/stop_reason").and_then(Value::as_str),
                                )
                                .to_string(),
                            ),
                            ..Default::default()
                        });
                    }
                    "message_stop" => {
                        yield Ok(ChatChunk {
                            done: true,
                            ..Default::default()
                        });
                        break;
                    }
                    "error" => {
                        let kind = data.pointer("/error/type").and_then(Value::as_str);
                        let message = data
                            .pointer("/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error");
                        yield Err(ProviderError::Other(match kind {
                            Some(kind) => format!("anthropic {kind}: {message}"),
                            None => format!("anthropic error: {message}"),
                        }));
                        break;
                    }
                    _ => {}
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
                        .header("x-api-key", key)
                        .header("anthropic-version", API_VERSION)
                        .send()
                        .await
                        .map_err(ProviderError::Transport)?;
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response);
                    }
                    let asked = retry_after(&response);
                    let body = response.text().await.unwrap_or_default();
                    Err(error_for_response(status.as_u16(), &body, asked))
                }
            };
            let response = match self.retry.run(send).await {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };

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
                    last_error = Some(ProviderError::Decode(format!("{err} (from {root}/models)")));
                    continue;
                }
            };

            let models = json["data"]
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| {
                            let id = entry.get("id").and_then(Value::as_str)?;
                            Some(ModelInfo {
                                id: id.to_string(),
                                owned_by: Some(
                                    entry
                                        .get("display_name")
                                        .and_then(Value::as_str)
                                        .unwrap_or("anthropic")
                                        .to_string(),
                                ),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return Ok(models);
        }

        Err(last_error.unwrap_or_else(|| ProviderError::Other("no API root reachable".into())))
    }
}
