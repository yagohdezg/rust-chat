//! Storage abstractions for rust-chat.
//!
//! Everything above this crate (server, agents) depends on the [`Store`] trait
//! rather than on `sqlx` or a specific engine. Concrete backends live in the
//! `chat-db` crate:
//!
//! * `chat-db` (`postgres` feature) — [`Store`] on Postgres.
//! * `chat-db` (`sqlite` feature)   — [`Store`] on SQLite.

pub mod files;
pub mod models;
pub mod traits;
pub mod types;

pub use files::{FileStore, LocalFileStore};
pub use models::{
    AdminUserSummary, Agent, AgentDraft, ApiKey, ApiKeyDraft, AuditEntry, AuditLog, AuthToken,
    AuthTokenDraft, Computer, Conversation, FileRecord, McpServer, McpServerDraft, Message,
    PoolBox, Provider, ProviderDraft, ProviderModel, RefreshToken, UsageByModel, UsageByUser,
    UsageDraft, UsageRecord, UsageSummary, User,
};
pub use traits::Store;
