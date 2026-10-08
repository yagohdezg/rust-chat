//! Postgres implementation of the `chat-store` traits, plus `pgvector` search.
//!
//! All SQL for this backend lives here. Swapping to another engine means
//! writing a sibling crate, not touching the application.

mod vector;

use chat_core::{ChatError, Result};
use chat_store::{Conversation, FileRecord, Message, Store, User};
use chrono::Utc;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

pub use vector::PgVectorStore;

/// Relational store backed by Postgres.
#[derive(Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Connect to Postgres and return a pooled handle.
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    /// Handle to the pool, for sharing with the vector store.
    pub fn pool(&self) -> PgPool {
        self.pool.clone()
    }

    /// Run embedded migrations from `migrations/postgres/`.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("../../migrations/postgres")
            .run(&self.pool)
            .await
            .map_err(|e| ChatError::Internal(anyhow::anyhow!("migration failed: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Store for PostgresStore {
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
             values ($1, $2, $3, $4, $5, $6, $7)
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
        let user = sqlx::query_as::<_, User>("select * from users where email = $1")
            .bind(email)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn get_user(&self, id: Uuid) -> Result<Option<User>> {
        let user = sqlx::query_as::<_, User>("select * from users where id = $1")
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
             values ($1, $2, $3, $4, $5, $6)
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
            "select * from conversations where user_id = $1 order by updated_at desc limit 200",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn get_conversation(&self, id: Uuid, user_id: Uuid) -> Result<Conversation> {
        let row = sqlx::query_as::<_, Conversation>(
            "select * from conversations where id = $1 and user_id = $2",
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
             values ($1, $2, $3, $4, $5, $6, $7, $8)
             returning *",
        )
        .bind(Uuid::new_v4())
        .bind(conversation_id)
        .bind(role)
        .bind(content)
        .bind(tool_calls)
        .bind(tool_call_id)
        .bind(status)
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    async fn update_message_content(&self, id: Uuid, content: &str, status: &str) -> Result<()> {
        sqlx::query("update messages set content = $2, status = $3 where id = $1")
            .bind(id)
            .bind(content)
            .bind(status)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_message(&self, id: Uuid) -> Result<Option<Message>> {
        let row = sqlx::query_as::<_, Message>("select * from messages where id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn list_messages(&self, conversation_id: Uuid) -> Result<Vec<Message>> {
        let rows = sqlx::query_as::<_, Message>(
            "select * from messages where conversation_id = $1 order by created_at asc limit 1000",
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
             values ($1, $2, $3, $4, $5, $6, $7, $8)
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
            sqlx::query_as::<_, FileRecord>("select * from files where id = $1 and user_id = $2")
                .bind(id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn list_files(&self, conversation_id: Uuid) -> Result<Vec<FileRecord>> {
        let rows = sqlx::query_as::<_, FileRecord>(
            "select * from files where conversation_id = $1 order by created_at asc",
        )
        .bind(conversation_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn delete_file(&self, id: Uuid, user_id: Uuid) -> Result<()> {
        sqlx::query("delete from files where id = $1 and user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
