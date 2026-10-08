use std::pin::Pin;
use std::sync::Arc;

use futures::{Stream, StreamExt};
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

/// Events emitted by the agent loop as it runs.
///
/// A turn is either a *tool turn* (the model requested one or more tools) or a
/// *final turn* (the model produced the answer). Tool-turn text is carried
/// whole on [`AgentEvent::ToolCalls`]; final-turn text is emitted as
/// [`AgentEvent::Delta`] so the caller can stream it.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Incremental text of the final assistant answer.
    Delta(String),
    /// The model finished an assistant turn that requested tool calls.
    /// `content` is that turn's text and `tool_calls` is the OpenAI-style JSON
    /// array (ready to persist on an assistant message).
    ToolCalls { content: String, tool_calls: Value },
    /// A single tool call finished executing; `content` is its output.
    ToolResult {
        id: String,
        name: String,
        content: String,
    },
}

#[derive(Default, Clone)]
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

/// Streaming variant of [`run_agent`].
///
/// Emits [`AgentEvent`]s as the loop progresses so the caller can forward
/// `delta`, `tool_call` and `tool_result` events to a client and persist tool
/// turns. Each iteration is buffered so a turn that ends up requesting tools is
/// never mistaken for the final answer.
pub fn run_agent_stream(
    provider: Arc<dyn LlmProvider>,
    tools: ToolRegistry,
    config: AgentConfig,
    mut messages: Vec<ChatMessage>,
) -> Pin<Box<dyn Stream<Item = anyhow::Result<AgentEvent>> + Send>> {
    Box::pin(async_stream::stream! {
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

            let mut stream = match provider.stream(request).await {
                Ok(stream) => stream,
                Err(err) => {
                    yield Err(anyhow::Error::new(err));
                    return;
                }
            };

            let mut content = String::new();
            let mut partial: Vec<PartialToolCall> = Vec::new();

            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => {
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
                    Err(err) => {
                        yield Err(anyhow::Error::new(err));
                        return;
                    }
                }
            }

            if partial.is_empty() {
                messages.push(ChatMessage::assistant(content.clone()));
                if !content.is_empty() {
                    yield Ok(AgentEvent::Delta(content));
                }
                return;
            }

            let calls = tool_calls_json(&partial);
            messages.push(ChatMessage {
                role: "assistant".into(),
                content: content.clone(),
                tool_calls: Some(calls.clone()),
                ..Default::default()
            });
            yield Ok(AgentEvent::ToolCalls { content, tool_calls: calls });

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
                messages.push(ChatMessage::tool(&call.id, result.clone()));
                yield Ok(AgentEvent::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    content: result,
                });
            }
        }

        let stopped = "[agent stopped: maximum tool iterations reached]".to_string();
        messages.push(ChatMessage::assistant(stopped.clone()));
        yield Ok(AgentEvent::Delta(stopped));
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::tool::Tool;
    use chat_providers::{ChatChunk, ChatStream, ProviderError};

    /// A provider that replays a scripted list of turns, one per call.
    struct ScriptedProvider {
        turns: Mutex<VecDeque<Vec<ChatChunk>>>,
    }

    impl ScriptedProvider {
        fn new(turns: Vec<Vec<ChatChunk>>) -> Self {
            Self {
                turns: Mutex::new(turns.into()),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for ScriptedProvider {
        fn name(&self) -> &str {
            "scripted"
        }

        async fn stream(
            &self,
            _request: ChatRequest,
        ) -> std::result::Result<ChatStream, ProviderError> {
            let chunks = self.turns.lock().unwrap().pop_front().unwrap_or_default();
            Ok(Box::pin(futures::stream::iter(chunks.into_iter().map(Ok))))
        }

        async fn list_models(
            &self,
        ) -> std::result::Result<Vec<chat_providers::ModelInfo>, ProviderError> {
            Ok(Vec::new())
        }
    }

    struct EchoTool;

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "returns a fixed string"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn call(&self, _arguments: Value) -> anyhow::Result<String> {
            Ok("hello".into())
        }
    }

    #[tokio::test]
    async fn stream_executes_tools_and_emits_events() {
        let provider = Arc::new(ScriptedProvider::new(vec![
            // First turn: request the `echo` tool.
            vec![
                ChatChunk {
                    tool_calls: Some(json!([{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "echo", "arguments": "{\"x\":1}" }
                    }])),
                    ..Default::default()
                },
                ChatChunk {
                    done: true,
                    ..Default::default()
                },
            ],
            // Second turn: final answer.
            vec![
                ChatChunk {
                    delta: "42".into(),
                    ..Default::default()
                },
                ChatChunk {
                    done: true,
                    ..Default::default()
                },
            ],
        ]));

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let config = AgentConfig {
            model: "test".into(),
            max_iterations: 4,
            ..Default::default()
        };

        let mut stream = run_agent_stream(
            provider,
            registry,
            config,
            vec![ChatMessage::user("what is the answer?")],
        );

        let mut tool_call = false;
        let mut tool_result = false;
        let mut final_text = String::new();
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::Delta(text) => final_text.push_str(&text),
                AgentEvent::ToolCalls { tool_calls, .. } => {
                    assert!(tool_calls.to_string().contains("echo"));
                    tool_call = true;
                }
                AgentEvent::ToolResult { name, content, .. } => {
                    assert_eq!(name, "echo");
                    assert_eq!(content, "hello");
                    tool_result = true;
                }
            }
        }

        assert!(tool_call, "expected a tool_call event");
        assert!(tool_result, "expected a tool_result event");
        assert_eq!(final_text, "42");
    }

    #[tokio::test]
    async fn stream_stops_at_iteration_budget() {
        // Always request a tool, never producing a final answer.
        let tool_turn = || {
            vec![
                ChatChunk {
                    tool_calls: Some(json!([{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "echo", "arguments": "{}" }
                    }])),
                    ..Default::default()
                },
                ChatChunk {
                    done: true,
                    ..Default::default()
                },
            ]
        };
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_turn(),
            tool_turn(),
            tool_turn(),
            tool_turn(),
        ]));

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        let config = AgentConfig {
            model: "test".into(),
            max_iterations: 2,
            ..Default::default()
        };

        let mut stream =
            run_agent_stream(provider, registry, config, vec![ChatMessage::user("loop")]);
        let mut final_text = String::new();
        while let Some(event) = stream.next().await {
            if let AgentEvent::Delta(text) = event.unwrap() {
                final_text.push_str(&text);
            }
        }
        assert_eq!(
            final_text,
            "[agent stopped: maximum tool iterations reached]"
        );
    }
}
