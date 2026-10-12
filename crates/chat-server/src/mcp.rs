//! MCP server registry.
//!
//! Owns the live `chat-mcp` stdio clients, keyed by server name, so the agent
//! runtime can list and call their tools. The registry starts empty and spawns
//! nothing: MCP server configuration and lifecycle are still pending, so this
//! is wiring only.

use std::collections::BTreeMap;

use tokio::sync::Mutex;

use chat_mcp::StdioMcpClient;

/// Live MCP clients, keyed by server name.
#[derive(Default)]
pub struct McpRegistry {
    clients: Mutex<BTreeMap<String, StdioMcpClient>>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of connected MCP servers.
    pub async fn server_count(&self) -> usize {
        self.clients.lock().await.len()
    }
}
