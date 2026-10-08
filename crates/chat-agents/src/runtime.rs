use futures::StreamExt;
use serde_json::{json, Value};

use chat_providers::{ChatMessage, ChatRequest, LlmProvider};

use crate::tool::ToolRegistry;

#[derive(Debug, Clone, Default)]
pub struct AgentConfig {
    pub model: String,
    pub max_iterations: usize,
    pub system: Option<String>,
    pub temperature: Option<f32>,
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Merge an OpenAI-style `tool_calls` delta array into the accumulator.
fn accumulate(calls: &mut Vec<PartialToolCall>, delta: &Value) {
    let Some(array) = delta.as_array() else {
        return;
    };
    for item in array {
        let index = item.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        while calls.len() <= index {
            calls.push(PartialToolCall::default());
        }
        let call = &mut calls[index];
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            if !id.is_empty() {
                call.id = id.to_string();
            }
        }
        if let Some(name) = item.pointer("/function/name").and_then(Value::as_str) {
            if !name.is_empty() {
                call.name = name.to_string();
            }
        }
        if let Some(args) = item.pointer("/function/arguments").and_then(Value::as_str) {
            call.arguments.push_str(args);
        }
    }
}

fn tool_calls_json(calls: &[PartialToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "type": "function",
                    "function": { "name": c.name, "arguments": c.arguments }
                })
            })
            .collect(),
    )
}

/// Run the agent loop until the model stops requesting tools or the iteration
/// budget is exhausted. Returns the full message list including tool results.
pub async fn run_agent(
    provider: &dyn LlmProvider,
    tools: &ToolRegistry,
    config: AgentConfig,
    mut messages: Vec<ChatMessage>,
) -> anyhow::Result<Vec<ChatMessage>> {
    if let Some(system) = &config.system {
        if messages.first().map(|m| m.role.as_str()) != Some("system") {
            messages.insert(0, ChatMessage::system(system.clone()));
        }
    }

    let tool_defs = if tools.is_empty() {
        None
    } else {
        Some(tools.definitions())
    };

    for _ in 0..config.max_iterations {
        let request = ChatRequest {
            model: config.model.clone(),
            messages: messages.clone(),
            temperature: config.temperature,
            max_tokens: None,
            tools: tool_defs.clone(),
        };

        let mut stream = provider.stream(request).await?;
        let mut content = String::new();
        let mut partial: Vec<PartialToolCall> = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if !chunk.delta.is_empty() {
                content.push_str(&chunk.delta);
            }
            if let Some(tc) = &chunk.tool_calls {
                accumulate(&mut partial, tc);
            }
            if chunk.done {
                break;
            }
        }

        if partial.is_empty() {
            messages.push(ChatMessage::assistant(content));
            return Ok(messages);
        }

        messages.push(ChatMessage {
            role: "assistant".into(),
            content: content.clone(),
            tool_calls: Some(tool_calls_json(&partial)),
            ..Default::default()
        });

        for call in &partial {
            let result = match tools.get(&call.name) {
                Some(tool) => {
                    let args: Value =
                        serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
                    match tool.call(args).await {
                        Ok(output) => output,
                        Err(err) => format!("tool `{}` failed: {err}", call.name),
                    }
                }
                None => format!("unknown tool: {}", call.name),
            };
            messages.push(ChatMessage::tool(&call.id, result));
        }
    }

    messages.push(ChatMessage::assistant(
        "[agent stopped: maximum tool iterations reached]",
    ));
    Ok(messages)
}
