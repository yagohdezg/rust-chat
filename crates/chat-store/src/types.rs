use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lifecycle values for [`crate::Message::status`].
///
/// Only assistant replies stream; other roles are always `complete`.
pub mod message_status {
    /// Generation finished successfully.
    pub const COMPLETE: &str = "complete";
    /// The model is still streaming into this message.
    pub const STREAMING: &str = "streaming";
    /// Generation failed with an error after partial output.
    pub const ERROR: &str = "error";
    /// The server stopped before generation finished (e.g. restart).
    pub const INTERRUPTED: &str = "interrupted";
}

/// Lifecycle values for [`crate::Computer::state`].
///
/// A computer starts out `provisioning` while the box is being placed, then
/// becomes `running`. `paused` keeps the placement but stops routing new work
/// until it is resumed; `destroyed` is terminal (the box is gone and the slot
/// is free).
pub mod computer_state {
    /// The box is being created on its node.
    pub const PROVISIONING: &str = "provisioning";
    /// The computer is live and serving executions.
    pub const RUNNING: &str = "running";
    /// The computer is idle but its placement is retained.
    pub const PAUSED: &str = "paused";
    /// Terminal: the box has been removed. The row is kept for audit.
    pub const DESTROYED: &str = "destroyed";
}

/// Scope for a vector search: always per-user, optionally narrowed to one
/// conversation.
#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub user_id: Uuid,
    pub conversation_id: Option<Uuid>,
}

/// A chunk of text together with its embedding, ready to persist.
#[derive(Debug, Clone)]
pub struct EmbeddingChunk {
    pub user_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub file_id: Option<Uuid>,
    pub content: String,
    pub embedding: Vec<f32>,
}

/// A chunk returned from a similarity search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievedChunk {
    pub content: String,
    pub file_id: Option<Uuid>,
    /// Cosine similarity in `[-1, 1]` (higher is more similar).
    pub score: f32,
}
