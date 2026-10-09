//! SQLite implementation of the `chat-store` traits.
//!
//! Useful for local development, tests, and single-file deployments. UUIDs are
//! BLOBs, timestamps are RFC3339 TEXT, JSON is TEXT.

use std::str::FromStr;
use std::sync::Arc;

use chat_core::{ChatError, Result, SecretCipher};
use chat_store::{
    AdminUserSummary, Agent, AgentDraft, AuditEntry, AuditLog, Computer, Conversation, FileRecord,
    Message, Provider, ProviderModel, RefreshToken, Store, User,
};
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::types::Json;
use sqlx::SqlitePool;
use uuid::Uuid;

/// Relational store backed by SQLite.
#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
    /// Encrypts/decrypts provider API keys at rest.
    secrets: Arc<SecretCipher>,
}

impl SqliteStore {
    pub fn new(pool: SqlitePool, secrets: Arc<SecretCipher>) -> Self {
        Self { pool, secrets }
    }

    /// Open (creating if needed) a SQLite database at `database_url`, e.g.
    /// `sqlite://.data/rustchat.db` or `sqlite::memory:`.
    pub async fn connect(database_url: &str, secrets: Arc<SecretCipher>) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(|e| ChatError::Config(format!("invalid SQLite URL: {e}")))?
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect_with(options)
            .await?;
        Ok(Self { pool, secrets })
    }

    /// Handle to the underlying pool.
    pub fn pool(&self) -> SqlitePool {
        self.pool.clone()
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
            sqlx::query("update providers set api_key = ? where id = ?")
                .bind(encrypted)
                .bind(id)
                .execute(&self.pool)
                .await?;
            rewritten += 1;
        }
        Ok(rewritten)
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
        sqlx::query("delete from users where id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_user_role(&self, id: Uuid, role: &str) -> Result<()> {
        sqlx::query("update users set role = ?, updated_at = ? where id = ?")
            .bind(role)
            .bind(Utc::now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_user_disabled(&self, id: Uuid, disabled: bool) -> Result<()> {
        sqlx::query("update users set disabled = ?, updated_at = ? where id = ?")
            .bind(disabled)
            .bind(Utc::now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn touch_user_last_seen(&self, id: Uuid) -> Result<()> {
        sqlx::query("update users set last_seen_at = ? where id = ?")
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
             values (?, ?, ?, ?, ?)",
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
            sqlx::query_as::<_, RefreshToken>("select * from refresh_tokens where token_hash = ?")
                .bind(token_hash)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn revoke_refresh_token(&self, token_hash: &str) -> Result<()> {
        sqlx::query(
            "update refresh_tokens set revoked_at = ? where token_hash = ? and revoked_at is null",
        )
        .bind(Utc::now())
        .bind(token_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn revoke_user_refresh_tokens(&self, user_id: Uuid) -> Result<()> {
        sqlx::query(
            "update refresh_tokens set revoked_at = ? where user_id = ? and revoked_at is null",
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
             values (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4())
        .bind(entry.actor_id)
        .bind(&entry.action)
        .bind(&entry.target_type)
        .bind(&entry.target_id)
        .bind(entry.metadata.clone().map(Json))
        .bind(&entry.ip)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_audit_logs(&self, limit: i64) -> Result<Vec<AuditLog>> {
        let rows = sqlx::query_as::<_, AuditLog>(
            "select * from audit_logs order by created_at desc limit ?",
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
            "select * from conversations where user_id = ?
             order by pinned desc, updated_at desc limit 200",
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

    async fn delete_conversation(&self, id: Uuid, user_id: Uuid) -> Result<()> {
        sqlx::query("delete from conversations where id = ? and user_id = ?")
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
            "update conversations set agent_id = ?, updated_at = ?
             where id = ? and user_id = ?
             returning *",
        )
        .bind(agent_id)
        .bind(Utc::now())
        .bind(id)
        .bind(user_id)
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
            "update conversations set title = ?, updated_at = ?
             where id = ? and user_id = ?
             returning *",
        )
        .bind(title)
        .bind(Utc::now())
        .bind(id)
        .bind(user_id)
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
            "update conversations set pinned = ?
             where id = ? and user_id = ?
             returning *",
        )
        .bind(pinned)
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ChatError::NotFound)?;
        Ok(row)
    }

    async fn duplicate_conversation(&self, id: Uuid, user_id: Uuid) -> Result<Conversation> {
        let mut tx = self.pool.begin().await?;
        let source = sqlx::query_as::<_, Conversation>(
            "select * from conversations where id = ? and user_id = ?",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ChatError::NotFound)?;

        let messages = sqlx::query_as::<_, Message>(
            "select * from messages where conversation_id = ? order by created_at asc",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;

        let new_id = Uuid::new_v4();
        let now = Utc::now();
        let row = sqlx::query_as::<_, Conversation>(
            "insert into conversations (id, user_id, agent_id, title, created_at, updated_at, pinned)
             values (?, ?, ?, ?, ?, ?, 0)
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

        for message in messages {
            sqlx::query(
                "insert into messages (id, conversation_id, role, content, tool_calls, tool_call_id, status, created_at)
                 values (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::new_v4())
            .bind(new_id)
            .bind(&message.role)
            .bind(&message.content)
            .bind(&message.tool_calls)
            .bind(&message.tool_call_id)
            .bind(&message.status)
            .bind(message.created_at)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(row)
    }

    async fn get_agent(&self, id: Uuid, user_id: Uuid) -> Result<Option<Agent>> {
        let row = sqlx::query_as::<_, Agent>("select * from agents where id = ? and user_id = ?")
            .bind(id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn list_agents(&self, user_id: Uuid) -> Result<Vec<Agent>> {
        let rows = sqlx::query_as::<_, Agent>(
            "select * from agents where user_id = ? order by created_at asc limit 200",
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
             values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        sqlx::query("delete from agents where id = ? and user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_provider(&self, id: Uuid, user_id: Uuid) -> Result<Option<Provider>> {
        let row = sqlx::query_as::<_, Provider>(
            "select * from providers where id = ? and (user_id = ? or user_id is null)",
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
             where user_id = ? or user_id is null
             order by (user_id = ?) desc, created_at asc",
        )
        .bind(user_id)
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
             values (?, ?, ?, ?, ?, ?, ?)
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
        sqlx::query("delete from providers where id = ? and user_id is ?")
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
             set name = ?, kind = ?, base_url = ?, api_key = ?
             where id = ?
             returning *",
        )
        .bind(name)
        .bind(kind)
        .bind(base_url)
        .bind(encrypted_key)
        .bind(id)
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
                     values (?, ?, ?, ?)
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
                    "delete from provider_credentials where user_id = ? and provider_id = ?",
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
            "select api_key from provider_credentials where user_id = ? and provider_id = ?",
        )
        .bind(user_id)
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(key,)| self.secrets.decrypt(&key)).transpose()
    }

    async fn list_provider_credential_ids(&self, user_id: Uuid) -> Result<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("select provider_id from provider_credentials where user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn list_provider_models(&self, provider_id: Uuid) -> Result<Vec<ProviderModel>> {
        let rows = sqlx::query_as::<_, ProviderModel>(
            "select provider_id, id, owned_by, fetched_at
             from models where provider_id = ? order by id asc",
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
        sqlx::query("delete from models where provider_id = ?")
            .bind(provider_id)
            .execute(&mut *tx)
            .await?;
        for model in models {
            sqlx::query(
                "insert into models (provider_id, id, owned_by, fetched_at)
                 values (?, ?, ?, ?)",
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

    async fn finalize_message(&self, id: Uuid, content: &str, status: &str) -> Result<()> {
        sqlx::query("update messages set content = ?, status = ?, created_at = ? where id = ?")
            .bind(content)
            .bind(status)
            .bind(Utc::now())
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
            "select f.* from files f
             join conversation_files cf on cf.file_id = f.id
             where cf.conversation_id = ?
             order by cf.created_at asc, f.created_at asc",
        )
        .bind(conversation_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn list_user_files(&self, user_id: Uuid) -> Result<Vec<FileRecord>> {
        let rows = sqlx::query_as::<_, FileRecord>(
            "select * from files where user_id = ? order by created_at desc",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn attach_file(&self, file_id: Uuid, conversation_id: Uuid, user_id: Uuid) -> Result<()> {
        sqlx::query(
            "insert or ignore into conversation_files (conversation_id, file_id, created_at)
             select ?, id, ? from files where id = ? and user_id = ?",
        )
        .bind(conversation_id)
        .bind(Utc::now())
        .bind(file_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn detach_file(&self, file_id: Uuid, conversation_id: Uuid, user_id: Uuid) -> Result<()> {
        sqlx::query(
            "delete from conversation_files
             where conversation_id = ? and file_id = ?
               and exists (select 1 from files where id = ? and user_id = ?)",
        )
        .bind(conversation_id)
        .bind(file_id)
        .bind(file_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_file(&self, id: Uuid, user_id: Uuid) -> Result<()> {
        sqlx::query("delete from files where id = ? and user_id = ?")
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
             values (?, ?, ?, ?, ?, ?, ?, ?)
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
        let row = sqlx::query_as::<_, Computer>("select * from computers where id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn get_live_computer_for_user(&self, user_id: Uuid) -> Result<Option<Computer>> {
        let row = sqlx::query_as::<_, Computer>(
            "select * from computers
             where user_id = ? and state != 'destroyed'
             order by created_at desc limit 1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn list_computers(&self, limit: i64) -> Result<Vec<Computer>> {
        let rows = sqlx::query_as::<_, Computer>(
            "select * from computers order by created_at desc limit ?",
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
             where state in ('running', 'paused') and last_active_at < ?
             order by last_active_at asc limit ?",
        )
        .bind(idle_before)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn count_live_computers_on_node(&self, node: &str) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>(
            "select count(*) from computers where node = ? and state != 'destroyed'",
        )
        .bind(node)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    async fn set_computer_state(&self, id: Uuid, state: &str) -> Result<()> {
        sqlx::query("update computers set state = ?, updated_at = ? where id = ?")
            .bind(state)
            .bind(Utc::now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_computer_handle(&self, id: Uuid, handle: Option<&str>) -> Result<()> {
        sqlx::query("update computers set handle = ?, updated_at = ? where id = ?")
            .bind(handle)
            .bind(Utc::now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn touch_computer(&self, id: Uuid) -> Result<()> {
        let now = Utc::now();
        sqlx::query("update computers set last_active_at = ?, updated_at = ? where id = ?")
            .bind(now)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
