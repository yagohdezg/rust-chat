//! Postgres implementation of the `chat-store` traits, plus `pgvector` search.
//!
//! All SQL for this backend lives here. Swapping to another engine means
//! writing a sibling crate, not touching the application.

mod vector;

use std::sync::Arc;

use chat_core::{ChatError, Result, SecretCipher};
use chat_store::{
    AdminUserSummary, Agent, AgentDraft, AuditEntry, AuditLog, Computer, Conversation, FileRecord,
    Message, Provider, ProviderModel, RefreshToken, Store, User,
};
use chrono::{DateTime, Utc};
use sqlx::postgres::PgPoolOptions;
use sqlx::types::Json;
use sqlx::PgPool;
use uuid::Uuid;

pub use vector::PgVectorStore;

/// Relational store backed by Postgres.
#[derive(Clone)]
pub struct PostgresStore {
    pool: PgPool,
    /// Encrypts/decrypts provider API keys at rest.
    secrets: Arc<SecretCipher>,
}

impl PostgresStore {
    pub fn new(pool: PgPool, secrets: Arc<SecretCipher>) -> Self {
        Self { pool, secrets }
    }

    /// Connect to Postgres and return a pooled handle.
    pub async fn connect(database_url: &str, secrets: Arc<SecretCipher>) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect(database_url)
            .await?;
        Ok(Self { pool, secrets })
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

    fn encrypt_opt(&self, plaintext: Option<&str>) -> Result<Option<String>> {
        plaintext.map(|v| self.secrets.encrypt(v)).transpose()
    }

    fn decrypt_provider(&self, mut provider: Provider) -> Result<Provider> {
        match self.secrets.decrypt_opt(provider.api_key.as_deref()) {
            Ok(key) => provider.api_key = key,
            Err(err) => {
                // A key sealed under a previous SECRET_ENCRYPTION_KEY/JWT_SECRET
                // cannot be recovered. Degrade to "no key" so one stranded row
                // does not 500 the whole provider list; rotate it to fix.
                tracing::warn!(
                    provider = %provider.id,
                    error = %err,
                    "provider API key could not be decrypted; treating it as unset"
                );
                provider.api_key = None;
            }
        }
        Ok(provider)
    }

    /// Encrypt provider API keys still stored as legacy plaintext. Idempotent;
    /// returns the number of rows rewritten.
    pub async fn encrypt_plaintext_provider_keys(&self) -> Result<u64> {
        let rows: Vec<(Uuid, String)> =
            sqlx::query_as("select id, api_key from providers where api_key is not null")
                .fetch_all(&self.pool)
                .await?;
        let mut rewritten = 0;
        for (id, key) in rows {
            if SecretCipher::is_encrypted(&key) {
                continue;
            }
            let encrypted = self.secrets.encrypt(&key)?;
            sqlx::query("update providers set api_key = $1 where id = $2")
                .bind(encrypted)
                .bind(id)
                .execute(&self.pool)
                .await?;
            rewritten += 1;
        }
        Ok(rewritten)
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

    async fn count_users(&self) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("select count(*) from users")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    async fn list_user_summaries(&self) -> Result<Vec<AdminUserSummary>> {
        let rows = sqlx::query_as::<_, AdminUserSummary>(
            "select u.id, u.email, u.name, u.role, u.disabled, u.created_at, u.last_seen_at,
                    (select count(*) from conversations c where c.user_id = u.id) as conversation_count,
                    (select count(*) from providers p where p.user_id = u.id) as provider_count,
                    (select count(*) from agents a where a.user_id = u.id) as agent_count
             from users u
             order by u.created_at asc",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn delete_user(&self, id: Uuid) -> Result<()> {
        sqlx::query("delete from users where id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_user_role(&self, id: Uuid, role: &str) -> Result<()> {
        sqlx::query("update users set role = $1, updated_at = $2 where id = $3")
            .bind(role)
            .bind(Utc::now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_user_disabled(&self, id: Uuid, disabled: bool) -> Result<()> {
        sqlx::query("update users set disabled = $1, updated_at = $2 where id = $3")
            .bind(disabled)
            .bind(Utc::now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn touch_user_last_seen(&self, id: Uuid) -> Result<()> {
        sqlx::query("update users set last_seen_at = $1 where id = $2")
            .bind(Utc::now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn create_refresh_token(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "insert into refresh_tokens (id, user_id, token_hash, expires_at, created_at)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(token_hash)
        .bind(expires_at)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_refresh_token(&self, token_hash: &str) -> Result<Option<RefreshToken>> {
        let row =
            sqlx::query_as::<_, RefreshToken>("select * from refresh_tokens where token_hash = $1")
                .bind(token_hash)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn revoke_refresh_token(&self, token_hash: &str) -> Result<()> {
        sqlx::query("update refresh_tokens set revoked_at = $1 where token_hash = $2 and revoked_at is null")
            .bind(Utc::now())
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn revoke_user_refresh_tokens(&self, user_id: Uuid) -> Result<()> {
        sqlx::query(
            "update refresh_tokens set revoked_at = $1 where user_id = $2 and revoked_at is null",
        )
        .bind(Utc::now())
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_audit(&self, entry: &AuditEntry) -> Result<()> {
        sqlx::query(
            "insert into audit_logs (id, actor_id, action, target_type, target_id, metadata, ip, created_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::new_v4())
        .bind(entry.actor_id)
        .bind(&entry.action)
        .bind(&entry.target_type)
        .bind(&entry.target_id)
        .bind(entry.metadata.clone())
        .bind(&entry.ip)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_audit_logs(&self, limit: i64) -> Result<Vec<AuditLog>> {
        let rows = sqlx::query_as::<_, AuditLog>(
            "select * from audit_logs order by created_at desc limit $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
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
            "select * from conversations where user_id = $1
             order by pinned desc, updated_at desc limit 200",
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

    async fn delete_conversation(&self, id: Uuid, user_id: Uuid) -> Result<()> {
        sqlx::query("delete from conversations where id = $1 and user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_conversation_agent(
        &self,
        id: Uuid,
        user_id: Uuid,
        agent_id: Option<Uuid>,
    ) -> Result<Conversation> {
        let row = sqlx::query_as::<_, Conversation>(
            "update conversations set agent_id = $3, updated_at = now()
             where id = $1 and user_id = $2
             returning *",
        )
        .bind(id)
        .bind(user_id)
        .bind(agent_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ChatError::NotFound)?;
        Ok(row)
    }

    async fn rename_conversation(
        &self,
        id: Uuid,
        user_id: Uuid,
        title: &str,
    ) -> Result<Conversation> {
        let row = sqlx::query_as::<_, Conversation>(
            "update conversations set title = $3, updated_at = now()
             where id = $1 and user_id = $2
             returning *",
        )
        .bind(id)
        .bind(user_id)
        .bind(title)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ChatError::NotFound)?;
        Ok(row)
    }

    async fn set_conversation_pinned(
        &self,
        id: Uuid,
        user_id: Uuid,
        pinned: bool,
    ) -> Result<Conversation> {
        let row = sqlx::query_as::<_, Conversation>(
            "update conversations set pinned = $3
             where id = $1 and user_id = $2
             returning *",
        )
        .bind(id)
        .bind(user_id)
        .bind(pinned)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ChatError::NotFound)?;
        Ok(row)
    }

    async fn duplicate_conversation(&self, id: Uuid, user_id: Uuid) -> Result<Conversation> {
        let mut tx = self.pool.begin().await?;
        let source = sqlx::query_as::<_, Conversation>(
            "select * from conversations where id = $1 and user_id = $2",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ChatError::NotFound)?;

        let new_id = Uuid::new_v4();
        let now = Utc::now();
        let row = sqlx::query_as::<_, Conversation>(
            "insert into conversations (id, user_id, agent_id, title, created_at, updated_at, pinned)
             values ($1, $2, $3, $4, $5, $6, false)
             returning *",
        )
        .bind(new_id)
        .bind(user_id)
        .bind(source.agent_id)
        .bind(format!("{} (copy)", source.title))
        .bind(now)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            "insert into messages (id, conversation_id, role, content, tool_calls, tool_call_id, status, created_at)
             select gen_random_uuid(), $1, role, content, tool_calls, tool_call_id, status, created_at
             from messages where conversation_id = $2 order by created_at",
        )
        .bind(new_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(row)
    }

    async fn get_agent(&self, id: Uuid, user_id: Uuid) -> Result<Option<Agent>> {
        let row = sqlx::query_as::<_, Agent>("select * from agents where id = $1 and user_id = $2")
            .bind(id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn list_agents(&self, user_id: Uuid) -> Result<Vec<Agent>> {
        let rows = sqlx::query_as::<_, Agent>(
            "select * from agents where user_id = $1 order by created_at asc limit 200",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn create_agent(&self, user_id: Uuid, draft: AgentDraft) -> Result<Agent> {
        let now = Utc::now();
        let row = sqlx::query_as::<_, Agent>(
            "insert into agents
             (id, user_id, name, instructions, provider_id, model, tools, sandbox_enabled, created_at, updated_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             returning *",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(draft.name)
        .bind(draft.instructions)
        .bind(draft.provider_id)
        .bind(draft.model)
        .bind(Json(draft.tools))
        .bind(draft.sandbox_enabled)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    async fn delete_agent(&self, id: Uuid, user_id: Uuid) -> Result<()> {
        sqlx::query("delete from agents where id = $1 and user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_provider(&self, id: Uuid, user_id: Uuid) -> Result<Option<Provider>> {
        let row = sqlx::query_as::<_, Provider>(
            "select * from providers where id = $1 and (user_id = $2 or user_id is null)",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|provider| self.decrypt_provider(provider))
            .transpose()
    }

    async fn list_providers(&self, user_id: Uuid) -> Result<Vec<Provider>> {
        let rows = sqlx::query_as::<_, Provider>(
            "select * from providers
             where user_id = $1 or user_id is null
             order by (user_id = $1) desc, created_at asc",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|provider| self.decrypt_provider(provider))
            .collect()
    }

    async fn create_provider(
        &self,
        owner_id: Option<Uuid>,
        name: &str,
        kind: &str,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Result<Provider> {
        let encrypted_key = self.encrypt_opt(api_key)?;
        let row = sqlx::query_as::<_, Provider>(
            "insert into providers (id, user_id, name, kind, base_url, api_key, created_at)
             values ($1, $2, $3, $4, $5, $6, $7)
             returning *",
        )
        .bind(Uuid::new_v4())
        .bind(owner_id)
        .bind(name)
        .bind(kind)
        .bind(base_url)
        .bind(encrypted_key)
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await?;
        self.decrypt_provider(row)
    }

    async fn delete_provider(&self, id: Uuid, owner_id: Option<Uuid>) -> Result<()> {
        sqlx::query("delete from providers where id = $1 and user_id is not distinct from $2")
            .bind(id)
            .bind(owner_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_provider(
        &self,
        id: Uuid,
        name: &str,
        kind: &str,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Result<Provider> {
        let encrypted_key = self.encrypt_opt(api_key)?;
        let row = sqlx::query_as::<_, Provider>(
            "update providers
             set name = $2, kind = $3, base_url = $4, api_key = $5
             where id = $1
             returning *",
        )
        .bind(id)
        .bind(name)
        .bind(kind)
        .bind(base_url)
        .bind(encrypted_key)
        .fetch_one(&self.pool)
        .await?;
        self.decrypt_provider(row)
    }

    async fn set_provider_credential(
        &self,
        user_id: Uuid,
        provider_id: Uuid,
        api_key: Option<&str>,
    ) -> Result<()> {
        match api_key {
            Some(key) => {
                let encrypted = self.secrets.encrypt(key)?;
                sqlx::query(
                    "insert into provider_credentials (user_id, provider_id, api_key, created_at)
                     values ($1, $2, $3, $4)
                     on conflict (user_id, provider_id)
                     do update set api_key = excluded.api_key",
                )
                .bind(user_id)
                .bind(provider_id)
                .bind(encrypted)
                .bind(Utc::now())
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query(
                    "delete from provider_credentials where user_id = $1 and provider_id = $2",
                )
                .bind(user_id)
                .bind(provider_id)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    async fn get_provider_credential(
        &self,
        user_id: Uuid,
        provider_id: Uuid,
    ) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            "select api_key from provider_credentials where user_id = $1 and provider_id = $2",
        )
        .bind(user_id)
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(key,)| self.secrets.decrypt(&key)).transpose()
    }

    async fn list_provider_credential_ids(&self, user_id: Uuid) -> Result<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("select provider_id from provider_credentials where user_id = $1")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn list_provider_models(&self, provider_id: Uuid) -> Result<Vec<ProviderModel>> {
        let rows = sqlx::query_as::<_, ProviderModel>(
            "select provider_id, id, owned_by, fetched_at
             from models where provider_id = $1 order by id asc",
        )
        .bind(provider_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn replace_provider_models(
        &self,
        provider_id: Uuid,
        models: &[ProviderModel],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("delete from models where provider_id = $1")
            .bind(provider_id)
            .execute(&mut *tx)
            .await?;
        for model in models {
            sqlx::query(
                "insert into models (provider_id, id, owned_by, fetched_at)
                 values ($1, $2, $3, $4)",
            )
            .bind(provider_id)
            .bind(&model.id)
            .bind(&model.owned_by)
            .bind(model.fetched_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
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

    async fn finalize_message(&self, id: Uuid, content: &str, status: &str) -> Result<()> {
        sqlx::query(
            "update messages set content = $2, status = $3, created_at = now() where id = $1",
        )
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

    async fn create_computer(
        &self,
        user_id: Uuid,
        node: &str,
        handle: Option<&str>,
    ) -> Result<Computer> {
        let now = Utc::now();
        let row = sqlx::query_as::<_, Computer>(
            "insert into computers
             (id, user_id, node, handle, state, created_at, updated_at, last_active_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8)
             returning *",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(node)
        .bind(handle)
        .bind(chat_store::types::computer_state::RUNNING)
        .bind(now)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    async fn get_computer(&self, id: Uuid) -> Result<Option<Computer>> {
        let row = sqlx::query_as::<_, Computer>("select * from computers where id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn get_live_computer_for_user(&self, user_id: Uuid) -> Result<Option<Computer>> {
        let row = sqlx::query_as::<_, Computer>(
            "select * from computers
             where user_id = $1 and state <> 'destroyed'
             order by created_at desc limit 1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn list_computers(&self, limit: i64) -> Result<Vec<Computer>> {
        let rows = sqlx::query_as::<_, Computer>(
            "select * from computers order by created_at desc limit $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn list_idle_computers(
        &self,
        idle_before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Computer>> {
        let rows = sqlx::query_as::<_, Computer>(
            "select * from computers
             where state in ('running', 'paused') and last_active_at < $1
             order by last_active_at asc limit $2",
        )
        .bind(idle_before)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn count_live_computers_on_node(&self, node: &str) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>(
            "select count(*) from computers where node = $1 and state <> 'destroyed'",
        )
        .bind(node)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    async fn set_computer_state(&self, id: Uuid, state: &str) -> Result<()> {
        sqlx::query("update computers set state = $2, updated_at = now() where id = $1")
            .bind(id)
            .bind(state)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_computer_handle(&self, id: Uuid, handle: Option<&str>) -> Result<()> {
        sqlx::query("update computers set handle = $2, updated_at = now() where id = $1")
            .bind(id)
            .bind(handle)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn touch_computer(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "update computers set last_active_at = now(), updated_at = now() where id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
