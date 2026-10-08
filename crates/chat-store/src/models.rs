use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;
use uuid::Uuid;

/// Domain model for a user account.
///
/// The structs here are storage-agnostic: they derive `sqlx::FromRow` (which is
/// implemented generically for every backend) and use `Json<T>` for JSON
/// columns so both Postgres and SQLite can decode them.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Uuid,
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: Option<String>,
    pub tool_calls: Option<Json<serde_json::Value>>,
    pub tool_call_id: Option<String>,
    /// Generation state for assistant messages: see [`crate::types::message_status`].
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub filename: String,
    pub mime: Option<String>,
    pub size_bytes: i64,
    pub storage_path: String,
    pub created_at: DateTime<Utc>,
}
