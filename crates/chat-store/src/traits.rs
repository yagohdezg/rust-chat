use chat_core::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::models::{
    AdminUserSummary, Agent, AgentDraft, AuditEntry, AuditLog, Computer, Conversation, FileRecord,
    Message, Provider, ProviderModel, RefreshToken, User,
};
use crate::types::{EmbeddingChunk, RetrievedChunk, Scope};

/// Relational persistence, independent of the database engine.
///
/// Implementations own their own SQL and migrations; the rest of the app only
/// ever sees this trait, so a new backend (SQLite, MySQL, ...) is an impl swap.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    // ---- users ---------------------------------------------------------
    async fn create_user(
        &self,
        email: &str,
        name: Option<&str>,
        password_hash: &str,
        role: &str,
    ) -> Result<User>;

    async fn find_user_by_email(&self, email: &str) -> Result<Option<User>>;

    async fn get_user(&self, id: Uuid) -> Result<Option<User>>;

    /// Number of accounts, used to make the very first registrant an admin.
    async fn count_users(&self) -> Result<i64>;

    /// Every account with access state and owned-resource counts (admin only).
    async fn list_user_summaries(&self) -> Result<Vec<AdminUserSummary>>;

    /// Delete an account. Cascades to its conversations, agents, providers,
    /// files and refresh tokens.
    async fn delete_user(&self, id: Uuid) -> Result<()>;

    /// Change an account's role (`user` | `admin`).
    async fn set_user_role(&self, id: Uuid, role: &str) -> Result<()>;

    /// Enable or disable an account. Disabled accounts are rejected on any
    /// authenticated request.
    async fn set_user_disabled(&self, id: Uuid, disabled: bool) -> Result<()>;

    /// Record best-effort activity for an account.
    async fn touch_user_last_seen(&self, id: Uuid) -> Result<()>;

    // ---- refresh tokens / audit ----------------------------------------
    /// Persist a refresh token by its hash (the plaintext never reaches the DB).
    async fn create_refresh_token(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<()>;

    /// Look up a refresh token by hash, revoked or not, so the caller can
    /// distinguish expiry from reuse of a already-rotated token.
    async fn get_refresh_token(&self, token_hash: &str) -> Result<Option<RefreshToken>>;

    /// Revoke a single refresh token (idempotent).
    async fn revoke_refresh_token(&self, token_hash: &str) -> Result<()>;

    /// Revoke every outstanding refresh token for a user (sign-out everywhere,
    /// or a precaution when a revoked token is replayed).
    async fn revoke_user_refresh_tokens(&self, user_id: Uuid) -> Result<()>;

    /// Append an entry to the security audit trail.
    async fn record_audit(&self, entry: &AuditEntry) -> Result<()>;

    /// The most recent audit entries, newest first.
    async fn list_audit_logs(&self, limit: i64) -> Result<Vec<AuditLog>>;

    // ---- conversations -------------------------------------------------
    async fn create_conversation(
        &self,
        user_id: Uuid,
        agent_id: Option<Uuid>,
        title: &str,
    ) -> Result<Conversation>;

    async fn list_conversations(&self, user_id: Uuid) -> Result<Vec<Conversation>>;

    async fn get_conversation(&self, id: Uuid, user_id: Uuid) -> Result<Conversation>;

    /// Delete a conversation owned by `user_id` and, via cascade, its messages
    /// and file rows. Blob cleanup is the caller's responsibility.
    async fn delete_conversation(&self, id: Uuid, user_id: Uuid) -> Result<()>;

    /// Rebind a conversation to `agent_id` (or clear it with `None`).
    async fn set_conversation_agent(
        &self,
        id: Uuid,
        user_id: Uuid,
        agent_id: Option<Uuid>,
    ) -> Result<Conversation>;

    /// Set a conversation's title.
    async fn rename_conversation(
        &self,
        id: Uuid,
        user_id: Uuid,
        title: &str,
    ) -> Result<Conversation>;

    /// Pin or unpin a conversation.
    async fn set_conversation_pinned(
        &self,
        id: Uuid,
        user_id: Uuid,
        pinned: bool,
    ) -> Result<Conversation>;

    /// Deep-copy a conversation — its metadata and all messages — under a new
    /// id, returning the copy.
    async fn duplicate_conversation(&self, id: Uuid, user_id: Uuid) -> Result<Conversation>;

    // ---- agents / providers --------------------------------------------
    /// Fetch an agent owned by `user_id` (used to build its runtime config).
    async fn get_agent(&self, id: Uuid, user_id: Uuid) -> Result<Option<Agent>>;

    /// List the calling user's agents, oldest first.
    async fn list_agents(&self, user_id: Uuid) -> Result<Vec<Agent>>;

    /// Create an agent owned by `user_id`.
    async fn create_agent(&self, user_id: Uuid, draft: AgentDraft) -> Result<Agent>;

    /// Delete an agent owned by `user_id`.
    async fn delete_agent(&self, id: Uuid, user_id: Uuid) -> Result<()>;

    /// Fetch a provider visible to `user_id`: their own or a global
    /// (admin-provided) one. Returns `None` when the id is unknown to them.
    async fn get_provider(&self, id: Uuid, user_id: Uuid) -> Result<Option<Provider>>;

    /// List providers visible to `user_id` — their own first, then globals.
    async fn list_providers(&self, user_id: Uuid) -> Result<Vec<Provider>>;

    /// Create a provider. `owner_id` is `None` for an admin-provided global
    /// provider, `Some(user)` for a personal one.
    async fn create_provider(
        &self,
        owner_id: Option<Uuid>,
        name: &str,
        kind: &str,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Result<Provider>;

    /// Delete a provider matching `owner_id` exactly (`None` = global).
    async fn delete_provider(&self, id: Uuid, owner_id: Option<Uuid>) -> Result<()>;

    /// Overwrite a provider's mutable fields (including its API key, encrypted
    /// at rest). Ownership is authorized by the caller.
    async fn update_provider(
        &self,
        id: Uuid,
        name: &str,
        kind: &str,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Result<Provider>;

    /// Store (or clear, with `None`) the caller's own API key for a provider.
    /// Used for admin-provisioned shared providers that ship without a key.
    async fn set_provider_credential(
        &self,
        user_id: Uuid,
        provider_id: Uuid,
        api_key: Option<&str>,
    ) -> Result<()>;

    /// The caller's decrypted API key for a provider, if any.
    async fn get_provider_credential(
        &self,
        user_id: Uuid,
        provider_id: Uuid,
    ) -> Result<Option<String>>;

    /// The ids of providers the caller has stored a key for.
    async fn list_provider_credential_ids(&self, user_id: Uuid) -> Result<Vec<Uuid>>;

    /// The cached model catalog for a provider, oldest fetch first. Empty when
    /// the provider has never been queried.
    async fn list_provider_models(&self, provider_id: Uuid) -> Result<Vec<ProviderModel>>;

    /// Replace a provider's cached catalog (delete-then-insert, atomically).
    async fn replace_provider_models(
        &self,
        provider_id: Uuid,
        models: &[ProviderModel],
    ) -> Result<()>;

    // ---- messages ------------------------------------------------------
    async fn insert_message(
        &self,
        conversation_id: Uuid,
        role: &str,
        content: Option<&str>,
        tool_calls: Option<serde_json::Value>,
        tool_call_id: Option<&str>,
    ) -> Result<Message>;

    /// Like [`Store::insert_message`] but with an explicit status, used to
    /// create an assistant placeholder before streaming.
    async fn insert_message_with_status(
        &self,
        conversation_id: Uuid,
        role: &str,
        content: Option<&str>,
        tool_calls: Option<serde_json::Value>,
        tool_call_id: Option<&str>,
        status: &str,
    ) -> Result<Message>;

    /// Replace a message's content and status (used to checkpoint a stream).
    async fn update_message_content(&self, id: Uuid, content: &str, status: &str) -> Result<()>;

    /// Replace a message's content and status and move it to the end of its
    /// conversation. Used to finalize an agent answer whose placeholder was
    /// created before the tool turns that logically precede it.
    async fn finalize_message(&self, id: Uuid, content: &str, status: &str) -> Result<()>;

    /// Fetch a single message by id (used to authorize and resume streams).
    async fn get_message(&self, id: Uuid) -> Result<Option<Message>>;

    async fn list_messages(&self, conversation_id: Uuid) -> Result<Vec<Message>>;

    // ---- files ---------------------------------------------------------
    async fn create_file(
        &self,
        user_id: Uuid,
        conversation_id: Option<Uuid>,
        filename: &str,
        mime: Option<&str>,
        size_bytes: i64,
        storage_path: &str,
    ) -> Result<FileRecord>;

    async fn get_file(&self, id: Uuid, user_id: Uuid) -> Result<Option<FileRecord>>;

    /// Files attached to a conversation (via the `conversation_files` join),
    /// oldest attachment first. This is the set materialized into that
    /// conversation's sandbox.
    async fn list_files(&self, conversation_id: Uuid) -> Result<Vec<FileRecord>>;

    /// Every file in the user's personal storage (library), newest first.
    async fn list_user_files(&self, user_id: Uuid) -> Result<Vec<FileRecord>>;

    /// Attach an existing library file to a conversation. The file must belong
    /// to `user_id`; re-attaching is a no-op.
    async fn attach_file(&self, file_id: Uuid, conversation_id: Uuid, user_id: Uuid) -> Result<()>;

    /// Detach a file from a conversation without deleting the library file.
    async fn detach_file(&self, file_id: Uuid, conversation_id: Uuid, user_id: Uuid) -> Result<()>;

    async fn delete_file(&self, id: Uuid, user_id: Uuid) -> Result<()>;

    // ---- computers (per-user persistent workspaces) --------------------
    /// Record a placement, creating a new `running` computer.
    async fn create_computer(
        &self,
        user_id: Uuid,
        node: &str,
        handle: Option<&str>,
    ) -> Result<Computer>;

    /// Fetch a single computer by id, regardless of owner or state.
    async fn get_computer(&self, id: Uuid) -> Result<Option<Computer>>;

    /// The user's live (non-destroyed) computer, newest first, if any.
    async fn get_live_computer_for_user(&self, user_id: Uuid) -> Result<Option<Computer>>;

    /// Every computer, newest first, capped at `limit` (admin/debug view).
    async fn list_computers(&self, limit: i64) -> Result<Vec<Computer>>;

    /// Live `running` computers whose `last_active_at` predates `idle_before`,
    /// oldest first — the idle reaper's work queue.
    async fn list_idle_computers(
        &self,
        idle_before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Computer>>;

    /// Count of live (non-destroyed) computers placed on `node`.
    async fn count_live_computers_on_node(&self, node: &str) -> Result<i64>;

    /// Change a computer's lifecycle state.
    async fn set_computer_state(&self, id: Uuid, state: &str) -> Result<()>;

    /// Set (or clear) the backend handle for a computer.
    async fn set_computer_handle(&self, id: Uuid, handle: Option<&str>) -> Result<()>;

    /// Mark a computer active now (refreshes the idle-reaper clock).
    async fn touch_computer(&self, id: Uuid) -> Result<()>;
}

/// Vector storage and similarity search, independent of the engine.
///
/// Postgres backs this with `pgvector`; SQLite uses a brute-force cosine scan
/// (fine for the small corpora of a personal deployment).
#[async_trait::async_trait]
pub trait VectorStore: Send + Sync {
    /// Delete every chunk previously indexed for a file (idempotent re-index).
    async fn delete_for_file(&self, file_id: Uuid) -> Result<()>;

    /// Persist a batch of embedded chunks.
    async fn insert_chunks(&self, chunks: &[EmbeddingChunk]) -> Result<()>;

    /// Return the `k` most similar chunks within `scope`.
    async fn search(&self, scope: &Scope, query: &[f32], k: usize) -> Result<Vec<RetrievedChunk>>;
}

/// Turns text into embedding vectors. Backed by an OpenAI-compatible
/// `/embeddings` endpoint, but swappable for a local model.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Dimensionality of the vectors this embedder produces.
    fn dimensions(&self) -> usize;

    /// Embed a batch of texts, preserving order.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}
