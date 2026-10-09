//! Storage abstractions for rust-chat.
//!
//! Everything above this crate (server, agents, RAG) depends on the
//! [`Store`], [`VectorStore`] and [`Embedder`] traits rather than on `sqlx` or
//! a specific engine. Concrete backends live in sibling crates:
//!
//! * `chat-db-postgres` — [`Store`] + [`VectorStore`] on Postgres/`pgvector`.
//! * `chat-db-sqlite`   — [`Store`] + [`VectorStore`] on SQLite.

pub mod files;
pub mod models;
pub mod traits;
pub mod types;

pub use files::{FileStore, LocalFileStore};
pub use models::{
    AdminUserSummary, Agent, AgentDraft, AuditEntry, AuditLog, Computer, Conversation, FileRecord,
    Message, Provider, ProviderModel, RefreshToken, User,
};
pub use traits::{Embedder, Store, VectorStore};
pub use types::{EmbeddingChunk, RetrievedChunk, Scope};
