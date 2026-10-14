use std::sync::Arc;

use chat_agents::ToolRegistry;
use chat_computers::ComputerOrchestrator;
use chat_core::Config;
use chat_sandbox::SandboxBackend;
use chat_store::{FileStore, Store};

use crate::mcp::McpRegistry;
use crate::rate_limit::RateLimiter;
use crate::scan::UploadScanner;
use crate::stream::StreamHub;
use crate::workspace::FileWorkspace;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    /// Relational persistence — one of the `chat-store` backends.
    pub store: Arc<dyn Store>,
    /// Uploaded file blob storage (local or S3-compatible).
    pub files: Arc<dyn FileStore>,
    /// Bridges the user's file library to the sandbox (in/out transfer).
    pub workspace: Arc<FileWorkspace>,
    pub sandbox: Arc<dyn SandboxBackend>,
    /// Per-user persistent "computer" control plane. `None` disables its routes.
    pub computers: Option<Arc<ComputerOrchestrator>>,
    /// Tools available to the agent runtime. Empty disables the agent path.
    pub tools: ToolRegistry,
    /// Live MCP clients whose tools the agent runtime can call.
    pub mcp: Arc<McpRegistry>,
    /// Registry of in-flight assistant generations, for resumable SSE.
    pub hub: Arc<StreamHub>,
    /// Process-local per-IP rate limiter for the auth and chat routes.
    pub rate_limit: Arc<RateLimiter>,
    /// Optional external malware scanner run on uploads.
    pub scanner: Arc<UploadScanner>,
}
