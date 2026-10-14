use std::collections::BTreeMap;

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
    /// Whether the account's email address has been confirmed. Only enforced
    /// when `REQUIRE_EMAIL_VERIFICATION` is on.
    pub email_verified: bool,
    /// Consecutive failed sign-ins; reset by a success or an admin unlock.
    pub failed_login_count: i64,
    /// End of the brute-force lock-out window, if the account is locked.
    pub locked_until: Option<DateTime<Utc>>,
    /// Models this account may use, as an allow-list. Empty means every model
    /// the provider allows.
    pub allowed_models: Json<Vec<String>>,
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
    pub locked_until: Option<DateTime<Utc>>,
    pub allowed_models: Json<Vec<String>>,
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
    /// Pinned chats sort ahead of the rest in listings.
    pub pinned: bool,
    /// Archived chats are hidden from the default listing.
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: Option<String>,
    /// Reasoning/thinking text a reasoning model emitted before its answer.
    pub reasoning: Option<String>,
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

/// A user-configured Model Context Protocol server.
///
/// A `stdio` server is spawned from `command` plus `args`/`env`; an `http`
/// server is reached at `url`. `enabled` gates whether its tools are exposed to
/// the agent runtime. `user_id` is `None` for a future admin-provided global.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct McpServer {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub name: String,
    /// `stdio` | `http`.
    pub transport: String,
    pub command: Option<String>,
    pub url: Option<String>,
    pub args: Json<Vec<String>>,
    pub env: Json<BTreeMap<String, String>>,
    /// Whether a `stdio` server runs inside the sandbox instead of on the host.
    pub sandbox: bool,
    /// Optional image override for a sandboxed server; `None` uses the
    /// deployment's `SANDBOX_IMAGE`.
    pub sandbox_image: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

/// Fields for creating an [`McpServer`].
#[derive(Debug, Clone, Default)]
pub struct McpServerDraft {
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub url: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub sandbox: bool,
    pub sandbox_image: Option<String>,
    pub enabled: bool,
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
    /// Whether this provider serves models that accept image input. Set from
    /// the provider form; decides whether attached images are forwarded to the
    /// model as image parts or read as text.
    pub vision: bool,
    /// Models this provider may serve, as an allow-list. Empty means the
    /// provider's whole discovered catalog is allowed.
    pub allowed_models: Json<Vec<String>>,
    pub created_at: DateTime<Utc>,
}

/// Fields for creating or overwriting a [`Provider`]. The id and timestamps are
/// assigned by the store.
#[derive(Debug, Clone, Default)]
pub struct ProviderDraft {
    /// `None` for an admin-provided global provider, `Some(user)` for a
    /// personal one.
    pub owner_id: Option<Uuid>,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    /// Plaintext key; the store encrypts it before writing.
    pub api_key: Option<String>,
    pub vision: bool,
    /// Model allow-list; empty means the provider's whole catalog.
    pub allowed_models: Vec<String>,
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

/// A single-use, time-limited token behind the email verification and password
/// reset flows. Only the SHA-256 `token_hash` is stored; `kind` separates the
/// two flows and `used_at` records consumption.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AuthToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// The fields needed to mint an auth token; id and timestamps are server-side.
#[derive(Debug, Clone)]
pub struct AuthTokenDraft {
    pub kind: String,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
}

/// A hashed, scoped API key for programmatic clients. The plaintext is shown
/// once at creation; only the SHA-256 `key_hash` is persisted, and `prefix`
/// stays visible so a key can be identified later.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub prefix: String,
    #[serde(skip_serializing)]
    pub key_hash: String,
    pub scopes: Json<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Fields for creating an [`ApiKey`]. The id and timestamps are assigned by
/// the store.
#[derive(Debug, Clone, Default)]
pub struct ApiKeyDraft {
    pub name: String,
    pub prefix: String,
    pub key_hash: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
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

/// A per-user, persistent execution workspace ("computer").
///
/// This row is the placement registry: it records which `node` hosts the
/// user's computer and its lifecycle `state`, so the control plane can route
/// executions without a sticky load balancer and can reconstruct placement
/// after a restart. `handle` is the backend's id for the running box (opaque
/// to everyone above the node client), and `volume` names the durable workspace
/// volume remounted whenever a box is (re)provisioned.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Computer {
    pub id: Uuid,
    pub user_id: Uuid,
    pub node: String,
    pub handle: Option<String>,
    /// Name of the durable workspace volume mounted into the box, if any.
    pub volume: Option<String>,
    /// See [`crate::types::computer_state`].
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
}

/// A blank, pre-provisioned box kept warm to hide BoxLite cold start.
///
/// The pool is durable ownership: the row records which node hosts the box, so
/// a restarted control plane can reconcile its warm pool against what a node
/// actually runs and reclaim anything it no longer owns.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PoolBox {
    pub handle: String,
    pub node: String,
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
