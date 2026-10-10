//! Domain types, configuration and errors shared across rust-chat crates.

pub mod config;
pub mod crypto;
pub mod error;
pub mod file_config;
pub mod ids;

pub use config::{Config, DatabaseBackend, FileStorageKind};
pub use crypto::SecretCipher;
pub use error::{ChatError, Result};
pub use file_config::{FileConfig, FileModelRef, FileProvider};
pub use ids::{AgentId, ConversationId, MessageId, UserId};
