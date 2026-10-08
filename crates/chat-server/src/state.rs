use std::sync::Arc;

use chat_agents::ToolRegistry;
use chat_core::Config;
use chat_rag::RagPipeline;
use chat_sandbox::SandboxBackend;
use chat_store::{FileStore, Store, VectorStore};

use crate::rate_limit::RateLimiter;
use crate::stream::StreamHub;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    /// Relational persistence — one of the `chat-store` backends.
    pub store: Arc<dyn Store>,
    /// Vector storage, shared by RAG and file-deletion cleanup.
    pub vectors: Arc<dyn VectorStore>,
    /// Uploaded file blob storage (local or S3-compatible).
    pub files: Arc<dyn FileStore>,
    pub sandbox: Arc<dyn SandboxBackend>,
    /// Tools available to the agent runtime. Empty disables the agent path.
    pub tools: ToolRegistry,
    /// RAG is optional; `None` when no embedding API key is configured.
    pub rag: Option<RagPipeline>,
    /// Registry of in-flight assistant generations, for resumable SSE.
    pub hub: Arc<StreamHub>,
    /// Process-local per-IP rate limiter for the auth and chat routes.
    pub rate_limit: Arc<RateLimiter>,
}
