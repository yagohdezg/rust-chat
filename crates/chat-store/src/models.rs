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
    /// Account lock-out flag set by an admin; enforced on every request.
    pub disabled: bool,
    /// Best-effort activity timestamp, refreshed in the background.
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// An account as seen by the admin console: profile, access state and counts of
/// the resources it owns.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AdminUserSummary {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub role: String,
    pub disabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub conversation_count: i64,
    pub provider_count: i64,
    pub agent_count: i64,
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

/// A user-defined agent: the instructions, model, and tools attached to a
/// conversation. `provider_id` may point at a user or global provider; when
/// absent the deployment default provider is used.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Agent {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub instructions: Option<String>,
    pub provider_id: Option<Uuid>,
    pub model: Option<String>,
    /// Names of the tools this agent may call (matched against the registry).
    pub tools: Json<Vec<String>>,
    pub sandbox_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// An OpenAI-compatible (or, later, Anthropic) model provider.
///
/// `user_id` is `None` for an admin-provided global provider that every user
/// inherits; `Some` scopes it to a single user.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Provider {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub name: String,
    /// `openai` | `anthropic` | `custom`.
    pub kind: String,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A model advertised by a provider, cached in the `models` table keyed by
/// `provider_id`. `fetched_at` drives the lazy-refresh TTL in the API layer.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProviderModel {
    pub provider_id: Uuid,
    pub id: String,
    pub owned_by: Option<String>,
    pub fetched_at: DateTime<Utc>,
}

/// A rotating refresh token. Only the SHA-256 `token_hash` is persisted; the
/// plaintext token lives with the client and is never stored server-side.
#[derive(Debug, Clone, FromRow)]
pub struct RefreshToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// One append-only entry in the security audit trail.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AuditLog {
    pub id: Uuid,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub metadata: Option<Json<serde_json::Value>>,
    pub ip: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Fields for appending an [`AuditLog`] entry. The id and timestamp are
/// assigned by the store.
#[derive(Debug, Clone, Default)]
pub struct AuditEntry {
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub ip: Option<String>,
}

/// Fields for creating an [`Agent`]. Kept as a struct so the `Store` method
/// stays within clippy's argument budget.
#[derive(Debug, Clone, Default)]
pub struct AgentDraft {
    pub name: String,
    pub instructions: Option<String>,
    pub provider_id: Option<Uuid>,
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub sandbox_enabled: bool,
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
