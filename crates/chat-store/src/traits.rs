use chat_core::Result;
use uuid::Uuid;

use crate::models::{Conversation, FileRecord, Message, User};
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

    // ---- conversations -------------------------------------------------
    async fn create_conversation(
        &self,
        user_id: Uuid,
        agent_id: Option<Uuid>,
        title: &str,
    ) -> Result<Conversation>;

    async fn list_conversations(&self, user_id: Uuid) -> Result<Vec<Conversation>>;

    async fn get_conversation(&self, id: Uuid, user_id: Uuid) -> Result<Conversation>;

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

    async fn list_files(&self, conversation_id: Uuid) -> Result<Vec<FileRecord>>;

    async fn delete_file(&self, id: Uuid, user_id: Uuid) -> Result<()>;
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
