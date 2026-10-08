//! Domain types, configuration and errors shared across rust-chat crates.

pub mod config;
pub mod error;
pub mod ids;

pub use config::{Config, DatabaseBackend, FileStorageKind, SandboxBackendKind};
pub use error::{ChatError, Result};
pub use ids::{AgentId, ConversationId, MessageId, UserId};
