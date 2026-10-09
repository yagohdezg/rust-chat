//! Storage abstractions for rust-chat.
//!
//! Everything above this crate (server, agents) depends on the [`Store`] trait
//! rather than on `sqlx` or a specific engine. Concrete backends live in
//! sibling crates:
//!
//! * `chat-db-postgres` — [`Store`] on Postgres.
//! * `chat-db-sqlite`   — [`Store`] on SQLite.

pub mod files;
pub mod models;
pub mod traits;
pub mod types;

pub use files::{FileStore, LocalFileStore};
pub use models::{
    AdminUserSummary, Agent, AgentDraft, AuditEntry, AuditLog, Computer, Conversation, FileRecord,
    Message, Provider, ProviderModel, RefreshToken, User,
};
pub use traits::Store;
