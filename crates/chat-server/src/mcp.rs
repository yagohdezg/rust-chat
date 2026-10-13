//! MCP server registry and tool adapter.
//!
//! Owns the live `chat-mcp` clients, keyed by server id, so the agent runtime
//! can list and call their tools. Clients are connected on demand from stored
//! user config, cached across turns, reconnected when the connection dies, and
//! shut down after an idle timeout.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use chat_agents::Tool;
use chat_mcp::{McpClient, McpError, McpServerConfig, McpTool, Transport};
use chat_store::McpServer;

/// How long a single MCP request (initialize, list, call) may take before it is
/// treated as a failure.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// A connected MCP server plus its cached tool catalog.
///
/// The live client is shared behind an `Arc` so concurrent tool calls can run in
/// parallel; `reconnect` serializes respawning when the connection dies.
struct McpClientHandle {
    name: String,
    config: McpServerConfig,
    client: RwLock<Arc<McpClient>>,
    reconnect: Mutex<()>,
    tools: Vec<McpTool>,
    last_used: Mutex<Instant>,
}

/// Translate stored config into a transport-agnostic connect config.
fn config_for(server: &McpServer) -> Result<McpServerConfig, McpError> {
    let transport = Transport::parse(&server.transport)
        .ok_or_else(|| McpError::Protocol(format!("unknown transport `{}`", server.transport)))?;
    Ok(McpServerConfig {
        name: server.name.clone(),
        transport,
        command: server.command.clone(),
        url: server.url.clone(),
        args: server.args.0.clone(),
        env: server.env.0.clone(),
    })
}

impl McpClientHandle {
    /// Connect, initialize and enumerate the tools of `server`.
    async fn spawn(server: &McpServer) -> Result<Self, McpError> {
        let config = config_for(server)?;
        let client = Arc::new(McpClient::connect(&config).await?);
        request(client.initialize()).await?;
        let tools = request(client.list_tools()).await?;

        Ok(Self {
            name: config.name.clone(),
            config,
            client: RwLock::new(client),
            reconnect: Mutex::new(()),
            tools,
            last_used: Mutex::new(Instant::now()),
        })
    }

    fn tool_list(&self) -> &[McpTool] {
        &self.tools
    }

    async fn touch(&self) {
        *self.last_used.lock().await = Instant::now();
    }

    async fn idle_for(&self) -> Duration {
        self.last_used.lock().await.elapsed()
    }

    /// Return the live client, reconnecting and re-initializing it if the
    /// connection died. Concurrent callers share one reconnect attempt.
    async fn ensure_alive(&self) -> Result<Arc<McpClient>, McpError> {
        let current = self.client.read().await.clone();
        if current.is_running() {
            return Ok(current);
        }

        let _guard = self.reconnect.lock().await;
        let current = self.client.read().await.clone();
        if current.is_running() {
            return Ok(current);
        }

        tracing::warn!(server = %self.name, "MCP server connection died; reconnecting");
        let fresh = Arc::new(McpClient::connect(&self.config).await?);
        request(fresh.initialize()).await?;
        *self.client.write().await = fresh.clone();
        Ok(fresh)
    }

    /// Call a tool, reconnecting and retrying once if the connection died.
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        let client = self.ensure_alive().await?;
        let first = request(client.call_tool(name, arguments.clone())).await;
        if first.is_ok() || client.is_running() {
            return first;
        }
        let client = self.ensure_alive().await?;
        request(client.call_tool(name, arguments)).await
    }
}

/// Run an MCP request under [`REQUEST_TIMEOUT`].
async fn request<T>(
    future: impl std::future::Future<Output = Result<T, McpError>>,
) -> Result<T, McpError> {
    tokio::time::timeout(REQUEST_TIMEOUT, future)
        .await
        .map_err(|_| McpError::Protocol("request timed out".into()))?
}

/// Live MCP clients, keyed by server id.
pub struct McpRegistry {
    servers: Mutex<HashMap<Uuid, Arc<McpClientHandle>>>,
    idle_ttl: Duration,
}

impl McpRegistry {
    pub fn new(idle_ttl: Duration) -> Self {
        Self {
            servers: Mutex::new(HashMap::new()),
            idle_ttl,
        }
    }

    /// Number of connected MCP servers.
    pub async fn server_count(&self) -> usize {
        self.servers.lock().await.len()
    }

    /// Connect (or reuse) the client for `server`.
    async fn connect(&self, server: &McpServer) -> Result<Arc<McpClientHandle>, McpError> {
        if let Some(handle) = self.servers.lock().await.get(&server.id) {
            handle.touch().await;
            return Ok(handle.clone());
        }

        let handle = Arc::new(McpClientHandle::spawn(server).await?);
        let mut servers = self.servers.lock().await;
        // A concurrent turn may have connected first; keep the cached one.
        let handle = servers.entry(server.id).or_insert(handle).clone();
        handle.touch().await;
        Ok(handle)
    }

    /// Build a `chat_agents::Tool` for every tool of every enabled server,
    /// connecting on demand. A server that fails to connect is skipped with a
    /// warning so one bad server never fails a whole turn.
    pub async fn tools_for(&self, servers: &[McpServer]) -> Vec<Arc<dyn Tool>> {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for server in servers.iter().filter(|server| server.enabled) {
            match self.connect(server).await {
                Ok(handle) => {
                    for tool in handle.tool_list() {
                        tools.push(Arc::new(McpToolAdapter::new(handle.clone(), tool.clone())));
                    }
                }
                Err(err) => {
                    tracing::warn!(server = %server.name, error = %err, "MCP server unavailable; skipping");
                }
            }
        }
        tools
    }

    /// Shut down servers idle for longer than the TTL, killing their processes.
    /// Returns the number stopped.
    pub async fn shutdown_idle(&self) -> usize {
        let mut servers = self.servers.lock().await;
        let mut idle = Vec::new();
        for (id, handle) in servers.iter() {
            if handle.idle_for().await >= self.idle_ttl {
                idle.push(*id);
            }
        }
        for id in &idle {
            servers.remove(id);
        }
        if !idle.is_empty() {
            tracing::info!(count = idle.len(), "stopped idle MCP servers");
        }
        idle.len()
    }
}

/// Wraps one MCP tool as a [`chat_agents::Tool`].
struct McpToolAdapter {
    handle: Arc<McpClientHandle>,
    tool: McpTool,
    name: String,
}

impl McpToolAdapter {
    fn new(handle: Arc<McpClientHandle>, tool: McpTool) -> Self {
        // Namespace by server so identically-named tools across servers do not
        // collide in the tool registry.
        let name = namespaced(&handle.name, &tool.name);
        Self { handle, tool, name }
    }
}

#[async_trait::async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.tool.description
    }

    fn schema(&self) -> Value {
        if self.tool.input_schema.is_null() {
            json!({ "type": "object", "properties": {} })
        } else {
            self.tool.input_schema.clone()
        }
    }

    async fn call(&self, arguments: Value) -> anyhow::Result<String> {
        self.handle.touch().await;
        let result = self.handle.call_tool(&self.tool.name, arguments).await?;
        Ok(format_tool_result(&result))
    }
}

/// A registry-safe tool name: `mcp__<server>__<tool>`, sanitized and capped at
/// the 64-character limit most providers enforce.
fn namespaced(server: &str, tool: &str) -> String {
    let mut name = format!("mcp__{}__{}", slug(server), slug(tool));
    name.truncate(64);
    name
}

fn slug(input: &str) -> String {
    let mut out: String = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push_str("server");
    }
    out
}

/// Render an MCP `tools/call` result as model-facing text. Text content blocks
/// are joined; anything else falls back to the raw JSON.
fn format_tool_result(result: &Value) -> String {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut text = String::new();
    if let Some(items) = result.get("content").and_then(Value::as_array) {
        for item in items {
            let Some(block) = item.get("text").and_then(Value::as_str) else {
                continue;
            };
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(block);
        }
    }
    if text.is_empty() {
        text = result.to_string();
    }
    if is_error {
        format!("error: {text}")
    } else {
        text
    }
}
