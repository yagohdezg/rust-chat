//! SQLite implementation of the `chat-store` traits, plus brute-force vector search.
//!
//! Useful for local development, tests, and single-file deployments. UUIDs are
//! BLOBs, timestamps are RFC3339 TEXT, JSON is TEXT.

mod vector;

use std::str::FromStr;

use chat_core::{ChatError, Result};
use chat_store::{Conversation, FileRecord, Message, Store, User};
use chrono::Utc;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::types::Json;
use sqlx::SqlitePool;
use uuid::Uuid;

pub use vector::SqliteVectorStore;

/// Relational store backed by SQLite.
#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Open (creating if needed) a SQLite database at `database_url`, e.g.
    /// `sqlite://.data/rustchat.db` or `sqlite::memory:`.
    pub async fn connect(database_url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(|e| ChatError::Config(format!("invalid SQLite URL: {e}")))?
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    /// Handle to the pool, for sharing with the vector store.
    pub fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    /// Run embedded migrations from `migrations/sqlite/`.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("../../migrations/sqlite")
            .run(&self.pool)
            .await
            .map_err(|e| ChatError::Internal(anyhow::anyhow!("migration failed: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Store for SqliteStore {
    async fn create_user(
        &self,
        email: &str,
        name: Option<&str>,
        password_hash: &str,
        role: &str,
    ) -> Result<User> {
        let now = Utc::now();
        let user = sqlx::query_as::<_, User>(
            "insert into users (id, email, name, password_hash, role, created_at, updated_at)
             values (?, ?, ?, ?, ?, ?, ?)
             returning *",
        )
        .bind(Uuid::new_v4())
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .bind(role)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(user)
    }

    async fn find_user_by_email(&self, email: &str) -> Result<Option<User>> {
        let user = sqlx::query_as::<_, User>("select * from users where email = ?")
            .bind(email)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn get_user(&self, id: Uuid) -> Result<Option<User>> {
        let user = sqlx::query_as::<_, User>("select * from users where id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn create_conversation(
        &self,
        user_id: Uuid,
        agent_id: Option<Uuid>,
        title: &str,
    ) -> Result<Conversation> {
        let now = Utc::now();
        let row = sqlx::query_as::<_, Conversation>(
            "insert into conversations (id, user_id, agent_id, title, created_at, updated_at)
             values (?, ?, ?, ?, ?, ?)
             returning *",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(agent_id)
        .bind(title)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    async fn list_conversations(&self, user_id: Uuid) -> Result<Vec<Conversation>> {
        let rows = sqlx::query_as::<_, Conversation>(
            "select * from conversations where user_id = ? order by updated_at desc limit 200",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn get_conversation(&self, id: Uuid, user_id: Uuid) -> Result<Conversation> {
        let row = sqlx::query_as::<_, Conversation>(
            "select * from conversations where id = ? and user_id = ?",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ChatError::NotFound)?;
        Ok(row)
    }

    async fn insert_message(
        &self,
        conversation_id: Uuid,
        role: &str,
        content: Option<&str>,
        tool_calls: Option<serde_json::Value>,
        tool_call_id: Option<&str>,
    ) -> Result<Message> {
        self.insert_message_with_status(
            conversation_id,
            role,
            content,
            tool_calls,
            tool_call_id,
            chat_store::types::message_status::COMPLETE,
        )
        .await
    }

    async fn insert_message_with_status(
        &self,
        conversation_id: Uuid,
        role: &str,
        content: Option<&str>,
        tool_calls: Option<serde_json::Value>,
        tool_call_id: Option<&str>,
        status: &str,
    ) -> Result<Message> {
        let row = sqlx::query_as::<_, Message>(
            "insert into messages (id, conversation_id, role, content, tool_calls, tool_call_id, status, created_at)
             values (?, ?, ?, ?, ?, ?, ?, ?)
             returning *",
        )
        .bind(Uuid::new_v4())
        .bind(conversation_id)
        .bind(role)
        .bind(content)
        .bind(tool_calls.map(Json))
        .bind(tool_call_id)
        .bind(status)
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    async fn update_message_content(&self, id: Uuid, content: &str, status: &str) -> Result<()> {
        sqlx::query("update messages set content = ?, status = ? where id = ?")
            .bind(content)
            .bind(status)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_message(&self, id: Uuid) -> Result<Option<Message>> {
        let row = sqlx::query_as::<_, Message>("select * from messages where id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn list_messages(&self, conversation_id: Uuid) -> Result<Vec<Message>> {
        let rows = sqlx::query_as::<_, Message>(
            "select * from messages where conversation_id = ? order by created_at asc limit 1000",
        )
        .bind(conversation_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn create_file(
        &self,
        user_id: Uuid,
        conversation_id: Option<Uuid>,
        filename: &str,
        mime: Option<&str>,
        size_bytes: i64,
        storage_path: &str,
    ) -> Result<FileRecord> {
        let row = sqlx::query_as::<_, FileRecord>(
            "insert into files (id, user_id, conversation_id, filename, mime, size_bytes, storage_path, created_at)
             values (?, ?, ?, ?, ?, ?, ?, ?)
             returning *",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(conversation_id)
        .bind(filename)
        .bind(mime)
        .bind(size_bytes)
        .bind(storage_path)
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    async fn get_file(&self, id: Uuid, user_id: Uuid) -> Result<Option<FileRecord>> {
        let row =
            sqlx::query_as::<_, FileRecord>("select * from files where id = ? and user_id = ?")
                .bind(id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn list_files(&self, conversation_id: Uuid) -> Result<Vec<FileRecord>> {
        let rows = sqlx::query_as::<_, FileRecord>(
            "select * from files where conversation_id = ? order by created_at asc",
        )
        .bind(conversation_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn delete_file(&self, id: Uuid, user_id: Uuid) -> Result<()> {
        sqlx::query("delete from files where id = ? and user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
