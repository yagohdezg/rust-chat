use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;
use uuid::Uuid;

use chat_agents::{run_agent_stream, AgentConfig, AgentEvent, ToolRegistry};
use chat_auth::{
    generate_refresh_token, hash_password, hash_refresh_token, issue_token, verify_password,
};
use chat_core::ChatError;
use chat_providers::{ChatMessage, ChatRequest, LlmProvider, ModelInfo, OpenAiProvider};
use chat_rag::RagPipeline;
use chat_sandbox::{ExecRequest, ExecResult, SandboxFile};
use chat_store::types::message_status;
use chat_store::{
    AdminUserSummary, AuditEntry, AuditLog, Conversation, FileRecord, Message, Provider,
    ProviderModel, Scope, Store,
};

use crate::error::ApiError;
use crate::extract::{AdminUser, AuthUser};
use crate::state::AppState;
use crate::stream::{SseEvent, StreamState};

/// Persist a streaming reply to the database every this many bytes.
const PERSIST_EVERY_BYTES: usize = 256;

/// How long a cached provider model catalog is considered fresh before the next
/// request triggers a refresh from the upstream `/models` endpoint.
const MODEL_CATALOG_TTL_SECONDS: i64 = 3600;

// ---- health ---------------------------------------------------------------

pub async fn health() -> &'static str {
    "ok"
}

// ---- auth -----------------------------------------------------------------

#[derive(Deserialize)]
pub struct RegisterBody {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub name: Option<String>,
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<RegisterBody>,
) -> Result<Json<Value>, ApiError> {
    if state.store.find_user_by_email(&body.email).await?.is_some() {
        return Err(ChatError::Conflict("email already registered".into()).into());
    }
    let hash = hash_password(&body.password)?;
    // The very first account becomes the admin who sets up the instance.
    let role = if state.store.count_users().await? == 0 {
        "admin"
    } else {
        "user"
    };
    let user = state
        .store
        .create_user(&body.email, body.name.as_deref(), &hash, role)
        .await
        .map_err(|err| conflict_on_unique(err, "email already registered"))?;
    let token = issue_token(
        &state.cfg.jwt_secret,
        user.id,
        &user.role,
        state.cfg.jwt_ttl_seconds,
    )?;
    let refresh_token = issue_refresh(&state, user.id).await?;

    audit(
        &state,
        AuditEntry {
            actor_id: Some(user.id),
            action: "auth.register".into(),
            target_type: Some("user".into()),
            target_id: Some(user.id.to_string()),
            metadata: None,
            ip: Some(addr.ip().to_string()),
        },
    )
    .await;

    Ok(Json(session_json(&user, &token, &refresh_token)))
}

#[derive(Deserialize)]
pub struct LoginBody {
    pub email: String,
    pub password: String,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<LoginBody>,
) -> Result<Json<Value>, ApiError> {
    let candidate = state.store.find_user_by_email(&body.email).await?;

    let valid = match candidate.as_ref() {
        Some(user) if user.password_hash.is_some() => {
            verify_password(&body.password, user.password_hash.as_deref().unwrap_or(""))
                .unwrap_or(false)
        }
        _ => false,
    };
    if !valid {
        audit(
            &state,
            AuditEntry {
                actor_id: candidate.as_ref().map(|u| u.id),
                action: "auth.login_failed".into(),
                target_type: Some("user".into()),
                target_id: candidate.as_ref().map(|u| u.id.to_string()),
                metadata: Some(json!({ "email": body.email })),
                ip: Some(addr.ip().to_string()),
            },
        )
        .await;
        return Err(ChatError::Unauthorized.into());
    }
    let user = candidate.expect("valid credentials imply a user row");
    if user.disabled {
        return Err(ChatError::Forbidden.into());
    }

    let token = issue_token(
        &state.cfg.jwt_secret,
        user.id,
        &user.role,
        state.cfg.jwt_ttl_seconds,
    )?;
    let refresh_token = issue_refresh(&state, user.id).await?;

    audit(
        &state,
        AuditEntry {
            actor_id: Some(user.id),
            action: "auth.login".into(),
            target_type: Some("user".into()),
            target_id: Some(user.id.to_string()),
            metadata: None,
            ip: Some(addr.ip().to_string()),
        },
    )
    .await;

    Ok(Json(session_json(&user, &token, &refresh_token)))
}

#[derive(Deserialize)]
pub struct RefreshBody {
    pub refresh_token: String,
}

/// `POST /api/auth/refresh` — exchange a refresh token for a fresh access token,
/// rotating (and revoking) the presented refresh token.
///
/// Replaying an already-revoked token is treated as a breach signal: every
/// outstanding token for that user is revoked so both the attacker and the
/// legitimate client are forced to re-authenticate.
pub async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RefreshBody>,
) -> Result<Json<Value>, ApiError> {
    let hash = hash_refresh_token(&body.refresh_token);
    let token = state
        .store
        .get_refresh_token(&hash)
        .await?
        .ok_or(ChatError::Unauthorized)?;

    if token.revoked_at.is_some() || token.expires_at <= Utc::now() {
        state
            .store
            .revoke_user_refresh_tokens(token.user_id)
            .await?;
        return Err(ChatError::Unauthorized.into());
    }

    let user = state
        .store
        .get_user(token.user_id)
        .await?
        .ok_or(ChatError::Unauthorized)?;
    if user.disabled {
        return Err(ChatError::Forbidden.into());
    }

    state.store.revoke_refresh_token(&hash).await?;
    let refresh_token = issue_refresh(&state, user.id).await?;
    let access = issue_token(
        &state.cfg.jwt_secret,
        user.id,
        &user.role,
        state.cfg.jwt_ttl_seconds,
    )?;
    Ok(Json(session_json(&user, &access, &refresh_token)))
}

#[derive(Deserialize)]
pub struct LogoutBody {
    #[serde(default)]
    pub refresh_token: Option<String>,
}

/// `POST /api/auth/logout` — revoke the presented refresh token. Idempotent;
/// access tokens simply expire on their own (they are stateless JWTs).
pub async fn logout(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LogoutBody>,
) -> Result<StatusCode, ApiError> {
    if let Some(token) = body.refresh_token.as_deref().filter(|t| !t.is_empty()) {
        state
            .store
            .revoke_refresh_token(&hash_refresh_token(token))
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn me(user: AuthUser) -> Json<Value> {
    Json(json!({ "id": user.id, "role": user.role }))
}

/// Issue and persist a new refresh token, returning the plaintext for the
/// client. The store only ever sees the SHA-256 hash.
async fn issue_refresh(state: &AppState, user_id: Uuid) -> Result<String, ApiError> {
    let token = generate_refresh_token()?;
    let expires = Utc::now() + Duration::seconds(state.cfg.refresh_ttl_seconds);
    state
        .store
        .create_refresh_token(user_id, &hash_refresh_token(&token), expires)
        .await?;
    Ok(token)
}

/// Append an audit entry, never failing the caller's request.
async fn audit(state: &AppState, entry: AuditEntry) {
    if let Err(err) = state.store.record_audit(&entry).await {
        tracing::warn!(error = %err, action = %entry.action, "failed to record audit entry");
    }
}

fn session_json(user: &chat_store::User, token: &str, refresh_token: &str) -> Value {
    json!({
        "token": token,
        "refresh_token": refresh_token,
        "user": { "id": user.id, "email": user.email, "name": user.name, "role": user.role }
    })
}

// ---- admin ----------------------------------------------------------------

/// `GET /api/admin/users` — every account with access state, last activity and
/// owned-resource counts.
pub async fn admin_list_users(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
) -> Result<Json<Vec<AdminUserSummary>>, ApiError> {
    Ok(Json(state.store.list_user_summaries().await?))
}

#[derive(Deserialize)]
pub struct UpdateUserBody {
    /// `user` | `admin`.
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub disabled: Option<bool>,
}

/// `PATCH /api/admin/users/{id}` — change an account's role and/or enabled
/// state. Admins cannot modify their own account (guards against lock-out).
pub async fn admin_update_user(
    State(state): State<Arc<AppState>>,
    admin: AdminUser,
    Path(user_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<UpdateUserBody>,
) -> Result<Json<chat_store::User>, ApiError> {
    if user_id == admin.0.id {
        return Err(ChatError::BadRequest("cannot modify your own account".into()).into());
    }
    if body.role.is_none() && body.disabled.is_none() {
        return Err(ChatError::BadRequest("nothing to update".into()).into());
    }
    let target = state
        .store
        .get_user(user_id)
        .await?
        .ok_or(ChatError::NotFound)?;

    // Don't let an admin lock everyone out by demoting/disabling the last one.
    let demoting = body.role.as_deref() == Some("user");
    let disabling = body.disabled == Some(true);
    if target.role == "admin"
        && !target.disabled
        && (demoting || disabling)
        && is_last_admin(&state, user_id).await?
    {
        return Err(ChatError::BadRequest("cannot remove the last remaining admin".into()).into());
    }

    if let Some(role) = body.role.as_deref() {
        if role != "user" && role != "admin" {
            return Err(ChatError::BadRequest("role must be `user` or `admin`".into()).into());
        }
        state.store.set_user_role(user_id, role).await?;
    }
    if let Some(disabled) = body.disabled {
        state.store.set_user_disabled(user_id, disabled).await?;
    }

    let user = state
        .store
        .get_user(user_id)
        .await?
        .ok_or(ChatError::NotFound)?;

    audit(
        &state,
        AuditEntry {
            actor_id: Some(admin.0.id),
            action: "admin.user.update".into(),
            target_type: Some("user".into()),
            target_id: Some(user_id.to_string()),
            metadata: Some(json!({ "role": body.role, "disabled": body.disabled })),
            ip: Some(addr.ip().to_string()),
        },
    )
    .await;

    Ok(Json(user))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /api/admin/audit?limit=` — recent security events, newest first.
pub async fn admin_list_audit(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
    Query(params): Query<AuditQuery>,
) -> Result<Json<Vec<AuditLog>>, ApiError> {
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    Ok(Json(state.store.list_audit_logs(limit).await?))
}

#[derive(Deserialize)]
pub struct CreateUserBody {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub name: Option<String>,
    /// `user` | `admin`; defaults to `user`.
    #[serde(default)]
    pub role: Option<String>,
}

/// `POST /api/admin/users` — create an account with an explicit role. Admin only.
pub async fn admin_create_user(
    State(state): State<Arc<AppState>>,
    admin: AdminUser,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<CreateUserBody>,
) -> Result<Json<chat_store::User>, ApiError> {
    let email = body.email.trim();
    if email.is_empty() || body.password.is_empty() {
        return Err(ChatError::BadRequest("email and password are required".into()).into());
    }
    let role = body.role.as_deref().unwrap_or("user");
    if role != "user" && role != "admin" {
        return Err(ChatError::BadRequest("role must be `user` or `admin`".into()).into());
    }
    if state.store.find_user_by_email(email).await?.is_some() {
        return Err(ChatError::Conflict("email already registered".into()).into());
    }
    let hash = hash_password(&body.password)?;
    let user = state
        .store
        .create_user(email, body.name.as_deref(), &hash, role)
        .await
        .map_err(|err| conflict_on_unique(err, "email already registered"))?;

    audit(
        &state,
        AuditEntry {
            actor_id: Some(admin.0.id),
            action: "admin.user.create".into(),
            target_type: Some("user".into()),
            target_id: Some(user.id.to_string()),
            metadata: Some(json!({ "email": user.email, "role": user.role })),
            ip: Some(addr.ip().to_string()),
        },
    )
    .await;

    Ok(Json(user))
}

/// `DELETE /api/admin/users/{id}` — delete an account. Cannot delete yourself or
/// the last remaining enabled admin.
pub async fn admin_delete_user(
    State(state): State<Arc<AppState>>,
    admin: AdminUser,
    Path(user_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<StatusCode, ApiError> {
    if user_id == admin.0.id {
        return Err(ChatError::BadRequest("cannot delete your own account".into()).into());
    }
    let target = state
        .store
        .get_user(user_id)
        .await?
        .ok_or(ChatError::NotFound)?;
    if target.role == "admin" && !target.disabled && is_last_admin(&state, user_id).await? {
        return Err(ChatError::BadRequest("cannot delete the last remaining admin".into()).into());
    }

    state.store.delete_user(user_id).await?;

    audit(
        &state,
        AuditEntry {
            actor_id: Some(admin.0.id),
            action: "admin.user.delete".into(),
            target_type: Some("user".into()),
            target_id: Some(user_id.to_string()),
            metadata: Some(json!({ "email": target.email })),
            ip: Some(addr.ip().to_string()),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Whether `user_id` is the only enabled admin left (locks are irreversible if
/// the last admin is removed).
async fn is_last_admin(state: &AppState, user_id: Uuid) -> Result<bool, ApiError> {
    let summaries = state.store.list_user_summaries().await?;
    let mut admins = summaries
        .into_iter()
        .filter(|u| u.role == "admin" && !u.disabled);
    let only = admins.next();
    let has_another = admins.next().is_some();
    Ok(!has_another && only.map(|u| u.id == user_id).unwrap_or(false))
}

// ---- models ---------------------------------------------------------------

/// `GET /api/models` — the models the caller's default provider can serve.
/// Powers a model picker and surfaces misconfiguration (e.g. an unknown default
/// model) before a chat request fails.
pub async fn list_models(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<ModelInfo>>, ApiError> {
    let row = default_provider_row(&state, user.id).await?;
    Ok(Json(models_for_provider(&state, &row, user.id).await?))
}

/// `GET /api/providers/{id}/models` — the cached model catalog for one
/// provider (own or global), refreshed lazily when stale.
pub async fn list_provider_models(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(provider_id): Path<Uuid>,
) -> Result<Json<Vec<ModelInfo>>, ApiError> {
    let row = state
        .store
        .get_provider(provider_id, user.id)
        .await?
        .ok_or(ChatError::NotFound)?;
    Ok(Json(models_for_provider(&state, &row, user.id).await?))
}

/// Return a provider's model catalog, serving the cached copy when fresh and
/// refreshing it from `list_models()` once the TTL expires. A failed refresh
/// falls back to the stale cache rather than erroring the picker.
async fn models_for_provider(
    state: &AppState,
    row: &Provider,
    user_id: Uuid,
) -> Result<Vec<ModelInfo>, ApiError> {
    let cached = state.store.list_provider_models(row.id).await?;
    if let Some(latest) = cached.first() {
        if !catalog_is_stale(latest.fetched_at) {
            return Ok(to_model_info(&cached));
        }
    }

    match build_provider(state, row, user_id)
        .await?
        .list_models()
        .await
    {
        Ok(models) => {
            let now = Utc::now();
            let rows: Vec<ProviderModel> = models
                .iter()
                .map(|m| ProviderModel {
                    provider_id: row.id,
                    id: m.id.clone(),
                    owned_by: m.owned_by.clone(),
                    fetched_at: now,
                })
                .collect();
            state.store.replace_provider_models(row.id, &rows).await?;
            Ok(models)
        }
        Err(err) if !cached.is_empty() => {
            tracing::warn!(
                provider = %row.id,
                error = %err,
                "model catalog refresh failed; serving stale cache"
            );
            Ok(to_model_info(&cached))
        }
        Err(err) => Err(ChatError::Upstream(err.to_string()).into()),
    }
}

fn catalog_is_stale(fetched_at: DateTime<Utc>) -> bool {
    Utc::now().signed_duration_since(fetched_at) > Duration::seconds(MODEL_CATALOG_TTL_SECONDS)
}

fn to_model_info(models: &[ProviderModel]) -> Vec<ModelInfo> {
    models
        .iter()
        .map(|m| ModelInfo {
            id: m.id.clone(),
            owned_by: m.owned_by.clone(),
        })
        .collect()
}

// ---- conversations --------------------------------------------------------

pub async fn list_conversations(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<Conversation>>, ApiError> {
    Ok(Json(state.store.list_conversations(user.id).await?))
}

#[derive(Deserialize)]
pub struct CreateConversationBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub agent_id: Option<Uuid>,
}

pub async fn create_conversation(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateConversationBody>,
) -> Result<Json<Conversation>, ApiError> {
    // An attached agent must belong to the caller; otherwise a user could
    // pin someone else's agent (and its provider key) onto a conversation.
    if let Some(agent_id) = body.agent_id {
        state
            .store
            .get_agent(agent_id, user.id)
            .await?
            .ok_or(ChatError::NotFound)?;
    }
    let title = body.title.unwrap_or_else(|| "New chat".into());
    Ok(Json(
        state
            .store
            .create_conversation(user.id, body.agent_id, &title)
            .await?,
    ))
}

pub async fn list_messages(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Vec<Message>>, ApiError> {
    state
        .store
        .get_conversation(conversation_id, user.id)
        .await?;
    Ok(Json(state.store.list_messages(conversation_id).await?))
}

pub async fn delete_conversation(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(conversation_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    // Authorize first: the conversation must belong to the caller.
    state
        .store
        .get_conversation(conversation_id, user.id)
        .await?;

    // Best-effort cleanup of stored blobs before the cascade drops the rows.
    match state.store.list_files(conversation_id).await {
        Ok(files) => {
            for file in files {
                if let Err(err) = state.files.delete(&file.storage_path).await {
                    tracing::warn!(error = %err, file = %file.id, "failed to delete stored blob");
                }
            }
        }
        Err(err) => tracing::warn!(error = %err, "failed to list files for deleted conversation"),
    }

    state
        .store
        .delete_conversation(conversation_id, user.id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct UpdateConversationBody {
    /// New title; omitted to leave it unchanged.
    #[serde(default)]
    pub title: Option<String>,
    /// New agent binding: omitted leaves it unchanged, `null` clears it.
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub agent_id: Option<Option<Uuid>>,
    /// Pin/unpin; omitted leaves it unchanged.
    #[serde(default)]
    pub pinned: Option<bool>,
}

/// Deserialize a present field into `Some(_)` even when its JSON value is
/// `null`, so a missing key (`None`) can be told apart from an explicit clear
/// (`Some(None)`).
fn deserialize_optional_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

pub async fn update_conversation(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(conversation_id): Path<Uuid>,
    Json(body): Json<UpdateConversationBody>,
) -> Result<Json<Conversation>, ApiError> {
    // The conversation must belong to the caller and, if set, the agent too.
    let mut conversation = state
        .store
        .get_conversation(conversation_id, user.id)
        .await?;

    if let Some(Some(agent_id)) = body.agent_id {
        state
            .store
            .get_agent(agent_id, user.id)
            .await?
            .ok_or(ChatError::NotFound)?;
    }

    if let Some(title) = body.title {
        let title = title.trim();
        if title.is_empty() {
            return Err(ChatError::BadRequest("conversation title is required".into()).into());
        }
        conversation = state
            .store
            .rename_conversation(conversation_id, user.id, title)
            .await?;
    }

    if let Some(agent_id) = body.agent_id {
        conversation = state
            .store
            .set_conversation_agent(conversation_id, user.id, agent_id)
            .await?;
    }

    if let Some(pinned) = body.pinned {
        conversation = state
            .store
            .set_conversation_pinned(conversation_id, user.id, pinned)
            .await?;
    }

    Ok(Json(conversation))
}

/// `POST /api/conversations/{id}/duplicate` — copy a conversation and all of
/// its messages under a new id. The copy is created unpinned.
pub async fn duplicate_conversation(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Conversation>, ApiError> {
    let conversation = state
        .store
        .duplicate_conversation(conversation_id, user.id)
        .await?;
    Ok(Json(conversation))
}

// ---- agents ---------------------------------------------------------------

pub async fn list_agents(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<chat_store::Agent>>, ApiError> {
    Ok(Json(state.store.list_agents(user.id).await?))
}

#[derive(Deserialize)]
pub struct CreateAgentBody {
    pub name: String,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub provider_id: Option<Uuid>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub sandbox_enabled: bool,
}

pub async fn create_agent(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateAgentBody>,
) -> Result<Json<chat_store::Agent>, ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ChatError::BadRequest("agent name is required".into()).into());
    }
    let blank_to_none = |value: Option<String>| value.filter(|v| !v.trim().is_empty());

    // A referenced provider must be visible to the caller (own or global).
    if let Some(provider_id) = body.provider_id {
        state
            .store
            .get_provider(provider_id, user.id)
            .await?
            .ok_or(ChatError::NotFound)?;
    }

    let agent = state
        .store
        .create_agent(
            user.id,
            chat_store::AgentDraft {
                name: name.to_string(),
                instructions: blank_to_none(body.instructions),
                provider_id: body.provider_id,
                model: blank_to_none(body.model),
                tools: body.tools,
                sandbox_enabled: body.sandbox_enabled,
            },
        )
        .await?;
    Ok(Json(agent))
}

pub async fn delete_agent(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(agent_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_agent(agent_id, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- providers ------------------------------------------------------------

/// A provider as seen by the calling user, with whether that user has a usable
/// key (their own credential, or a key the provider itself carries).
#[derive(Serialize)]
pub struct ProviderView {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub created_at: DateTime<Utc>,
    pub has_key: bool,
}

fn provider_view(provider: Provider, has_key: bool) -> ProviderView {
    ProviderView {
        id: provider.id,
        user_id: provider.user_id,
        name: provider.name,
        kind: provider.kind,
        base_url: provider.base_url,
        created_at: provider.created_at,
        has_key,
    }
}

/// The API key to use for `user_id` on `row`: the caller's own credential if
/// set, else the provider's own key (e.g. an admin-provisioned shared key).
async fn effective_api_key(
    state: &AppState,
    row: &Provider,
    user_id: Uuid,
) -> Result<Option<String>, ApiError> {
    if let Some(key) = state.store.get_provider_credential(user_id, row.id).await? {
        return Ok(Some(key));
    }
    Ok(row.api_key.clone())
}

/// Build a runtime client from a stored provider row, resolving the caller's
/// effective key.
///
/// Only OpenAI-compatible endpoints are supported today; an Anthropic-native
/// provider is a future addition (PLAN §3).
async fn build_provider(
    state: &AppState,
    row: &Provider,
    user_id: Uuid,
) -> Result<Arc<dyn LlmProvider>, ApiError> {
    let key = effective_api_key(state, row, user_id).await?;
    match row.kind.as_str() {
        "openai" | "custom" => Ok(Arc::new(OpenAiProvider::new(
            &row.base_url,
            key.unwrap_or_default(),
        ))),
        other => {
            Err(ChatError::Config(format!("provider kind `{other}` is not supported yet")).into())
        }
    }
}

/// Map a database unique-constraint violation (e.g. a duplicate provider name
/// or email) to a 409 rather than letting it surface as a 500.
fn conflict_on_unique(err: ChatError, message: &str) -> ApiError {
    if let ChatError::Database(db) = &err {
        if db
            .as_database_error()
            .map(|e| e.is_unique_violation())
            .unwrap_or(false)
        {
            return ChatError::Conflict(message.into()).into();
        }
    }
    err.into()
}

/// The provider to use when neither the request nor an agent names one: the
/// caller's first own provider, else the first admin-provided global one.
async fn default_provider(
    state: &AppState,
    user_id: Uuid,
) -> Result<Arc<dyn LlmProvider>, ApiError> {
    let row = default_provider_row(state, user_id).await?;
    build_provider(state, &row, user_id).await
}

/// The stored row behind [`default_provider`], for callers that need the id
/// (e.g. the cached model catalog).
async fn default_provider_row(state: &AppState, user_id: Uuid) -> Result<Provider, ApiError> {
    state
        .store
        .list_providers(user_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ChatError::ProviderNotConfigured.into())
}

pub async fn list_providers(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> Result<Json<Vec<ProviderView>>, ApiError> {
    let providers = state.store.list_providers(user.id).await?;
    let with_credential = state.store.list_provider_credential_ids(user.id).await?;
    let views = providers
        .into_iter()
        .map(|provider| {
            let has_key = provider.api_key.is_some() || with_credential.contains(&provider.id);
            provider_view(provider, has_key)
        })
        .collect();
    Ok(Json(views))
}

#[derive(Deserialize)]
pub struct CreateProviderBody {
    pub name: String,
    pub base_url: String,
    /// `openai` | `custom` (Anthropic is not supported yet).
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub api_key: Option<String>,
    /// Admin-only: expose this provider to every user.
    #[serde(default)]
    pub global: bool,
}

fn default_kind() -> String {
    "openai".into()
}

pub async fn create_provider(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateProviderBody>,
) -> Result<Json<ProviderView>, ApiError> {
    let name = body.name.trim();
    let base_url = body.base_url.trim();
    if name.is_empty() || base_url.is_empty() {
        return Err(ChatError::BadRequest("provider name and base_url are required".into()).into());
    }
    if body.global && user.role != "admin" {
        return Err(ChatError::Forbidden.into());
    }
    let owner = if body.global { None } else { Some(user.id) };
    let api_key = body
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());

    let provider = state
        .store
        .create_provider(owner, name, &body.kind, base_url, api_key)
        .await
        .map_err(|err| conflict_on_unique(err, "a provider with that name already exists"))?;
    let has_key = provider.api_key.is_some();
    Ok(Json(provider_view(provider, has_key)))
}

/// Whether `user` may modify or delete `provider`: their own, or a global
/// (admin-provided) one for admins.
fn can_manage_provider(provider: &Provider, user: &AuthUser) -> bool {
    match provider.user_id {
        Some(owner) => owner == user.id,
        None => user.role == "admin",
    }
}

#[derive(Deserialize)]
pub struct UpdateProviderBody {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub kind: Option<String>,
    /// Omit to keep the stored key; send an empty string to clear it.
    pub api_key: Option<String>,
}

/// `PUT /api/providers/{id}` — update a provider's fields (own provider, or a
/// global one for admins). Rotating base_url/kind/key invalidates the cached
/// model catalog.
pub async fn update_provider(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(provider_id): Path<Uuid>,
    Json(body): Json<UpdateProviderBody>,
) -> Result<Json<ProviderView>, ApiError> {
    let existing = state
        .store
        .get_provider(provider_id, user.id)
        .await?
        .ok_or(ChatError::NotFound)?;
    if !can_manage_provider(&existing, &user) {
        return Err(ChatError::Forbidden.into());
    }

    let pick = |value: Option<&str>, fallback: &str| -> String {
        value
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| fallback.to_owned())
    };
    let name = pick(body.name.as_deref(), &existing.name);
    let base_url = pick(body.base_url.as_deref(), &existing.base_url);
    let kind = pick(body.kind.as_deref(), &existing.kind);
    let api_key = match body.api_key.as_deref().map(str::trim) {
        None => existing.api_key.clone(),
        Some("") => None,
        Some(key) => Some(key.to_string()),
    };

    let provider = state
        .store
        .update_provider(provider_id, &name, &kind, &base_url, api_key.as_deref())
        .await
        .map_err(|err| conflict_on_unique(err, "a provider with that name already exists"))?;
    state
        .store
        .replace_provider_models(provider_id, &[])
        .await?;
    let has_key = provider.api_key.is_some();
    Ok(Json(provider_view(provider, has_key)))
}

#[derive(Deserialize)]
pub struct ProviderCredentialBody {
    /// Omit or send an empty string to clear the stored key.
    #[serde(default)]
    pub api_key: Option<String>,
}

/// `PUT /api/providers/{id}/credential` — store the caller's own key for a
/// provider. Lets users satisfy an admin-provisioned provider that ships
/// without a key; the credential is personal and encrypted at rest.
pub async fn set_provider_credential(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(provider_id): Path<Uuid>,
    Json(body): Json<ProviderCredentialBody>,
) -> Result<Json<ProviderView>, ApiError> {
    let provider = state
        .store
        .get_provider(provider_id, user.id)
        .await?
        .ok_or(ChatError::NotFound)?;
    let key = body
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    state
        .store
        .set_provider_credential(user.id, provider_id, key)
        .await?;
    // A freshly stored credential makes the provider usable from the caller's
    // perspective even when the provider itself carries no key.
    let has_key = key.is_some() || provider.api_key.is_some();
    Ok(Json(provider_view(provider, has_key)))
}

#[derive(Deserialize)]
pub struct BulkCreateProvidersBody {
    pub providers: Vec<CreateProviderBody>,
}

/// `POST /api/admin/providers` — create several instance-wide (global)
/// providers in one request. Admin only.
pub async fn admin_bulk_create_providers(
    State(state): State<Arc<AppState>>,
    _admin: AdminUser,
    Json(body): Json<BulkCreateProvidersBody>,
) -> Result<Json<Vec<ProviderView>>, ApiError> {
    if body.providers.is_empty() {
        return Err(ChatError::BadRequest("providers must not be empty".into()).into());
    }
    let mut created = Vec::with_capacity(body.providers.len());
    for entry in &body.providers {
        let name = entry.name.trim();
        let base_url = entry.base_url.trim();
        if name.is_empty() || base_url.is_empty() {
            return Err(
                ChatError::BadRequest("provider name and base_url are required".into()).into(),
            );
        }
        let api_key = entry
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty());
        let provider = state
            .store
            .create_provider(None, name, &entry.kind, base_url, api_key)
            .await
            .map_err(|err| conflict_on_unique(err, "a provider with that name already exists"))?;
        let has_key = provider.api_key.is_some();
        created.push(provider_view(provider, has_key));
    }
    Ok(Json(created))
}

pub async fn delete_provider(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(provider_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let provider = state
        .store
        .get_provider(provider_id, user.id)
        .await?
        .ok_or(ChatError::NotFound)?;
    if !can_manage_provider(&provider, &user) {
        return Err(ChatError::Forbidden.into());
    }
    state
        .store
        .delete_provider(provider_id, provider.user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- files (upload + download + delete + RAG indexing) --------------------

#[derive(Deserialize)]
pub struct UploadFileBody {
    pub filename: String,
    #[serde(default)]
    pub mime: Option<String>,
    /// Base64-encoded file contents.
    pub content_b64: String,
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
}

pub async fn upload_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<UploadFileBody>,
) -> Result<Json<Value>, ApiError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&body.content_b64)
        .map_err(|e| ChatError::BadRequest(format!("invalid base64 content: {e}")))?;

    if let Some(conversation_id) = body.conversation_id {
        state
            .store
            .get_conversation(conversation_id, user.id)
            .await?;
    }

    let file_id = Uuid::new_v4();
    let key = format!(
        "{}/{}-{}",
        user.id,
        file_id,
        sanitize_filename(&body.filename)
    );
    let location = state.files.put(&key, &bytes).await?;

    let record = state
        .store
        .create_file(
            user.id,
            body.conversation_id,
            &body.filename,
            body.mime.as_deref(),
            bytes.len() as i64,
            &location,
        )
        .await?;

    // Index into the vector store when RAG is enabled. Failures here do not
    // fail the upload — the file is already stored.
    let mut chunks_indexed = 0usize;
    let mut indexing_error: Option<String> = None;
    if let Some(rag) = &state.rag {
        match rag
            .index_file(user.id, body.conversation_id, record.id, &bytes)
            .await
        {
            Ok(n) => chunks_indexed = n,
            Err(err) => {
                tracing::warn!(error = %err, file = %record.filename, "indexing failed");
                indexing_error = Some(err.to_string());
            }
        }
    }

    Ok(Json(json!({
        "file": record,
        "rag_enabled": state.rag.is_some(),
        "chunks_indexed": chunks_indexed,
        "indexing_error": indexing_error,
    })))
}

pub async fn list_files(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Vec<FileRecord>>, ApiError> {
    state
        .store
        .get_conversation(conversation_id, user.id)
        .await?;
    Ok(Json(state.store.list_files(conversation_id).await?))
}

pub async fn download_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(file_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let file = state
        .store
        .get_file(file_id, user.id)
        .await?
        .ok_or(ChatError::NotFound)?;
    let bytes = state.files.get(&file.storage_path).await?;

    let mut headers = HeaderMap::new();
    let content_type = file
        .mime
        .clone()
        .unwrap_or_else(|| "application/octet-stream".into());
    if let Ok(value) = content_type.parse() {
        headers.insert(header::CONTENT_TYPE, value);
    }
    let disposition = format!(
        "attachment; filename=\"{}\"",
        file.filename.replace(['"', '\\'], "_")
    );
    if let Ok(value) = disposition.parse() {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    Ok((headers, bytes).into_response())
}

pub async fn delete_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(file_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let file = state
        .store
        .get_file(file_id, user.id)
        .await?
        .ok_or(ChatError::NotFound)?;

    // Best-effort cleanup of the blob and its embeddings, then the row.
    if let Err(err) = state.files.delete(&file.storage_path).await {
        tracing::warn!(error = %err, file = %file.id, "failed to delete stored blob");
    }
    if let Err(err) = state.vectors.delete_for_file(file.id).await {
        tracing::warn!(error = %err, file = %file.id, "failed to delete embeddings");
    }
    state.store.delete_file(file_id, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- chat (SSE, resumable) ------------------------------------------------

#[derive(Deserialize)]
pub struct ChatBody {
    pub conversation_id: Uuid,
    pub content: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider_id: Option<Uuid>,
}

pub async fn chat(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<ChatBody>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let conversation = state
        .store
        .get_conversation(body.conversation_id, user.id)
        .await?;

    state
        .store
        .insert_message(conversation.id, "user", Some(&body.content), None, None)
        .await?;

    let history = state.store.list_messages(conversation.id).await?;
    let mut messages: Vec<ChatMessage> = history
        .iter()
        .filter(|m| m.status != message_status::STREAMING)
        .map(|m| ChatMessage {
            role: m.role.clone(),
            content: m.content.clone().unwrap_or_default(),
            tool_calls: m.tool_calls.as_ref().map(|calls| calls.0.clone()),
            tool_call_id: m.tool_call_id.clone(),
            name: None,
        })
        .collect();

    // Retrieval-augmented generation: prepend the most relevant chunks.
    let mut sources: Vec<Value> = Vec::new();
    if let Some(rag) = &state.rag {
        let scope = Scope {
            user_id: user.id,
            conversation_id: Some(conversation.id),
        };
        match rag.retrieve(&scope, &body.content).await {
            Ok(chunks) => {
                if !chunks.is_empty() {
                    messages.insert(0, ChatMessage::system(RagPipeline::format_context(&chunks)));
                    sources = chunks
                        .iter()
                        .map(|c| json!({ "file_id": c.file_id, "score": c.score }))
                        .collect();
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "RAG retrieval failed; continuing without context")
            }
        }
    }

    let requested_model = body.model;
    let requested_provider = body.provider_id;

    // Route through the agent loop when the conversation has an agent attached
    // or any tools are registered; otherwise keep the lean single-shot path.
    // Resolve the agent *before* persisting the placeholder so a missing or
    // foreign agent fails the request without leaving a dangling stream.
    let use_agent = !state.tools.is_empty() || conversation.agent_id.is_some();
    let resolved = if use_agent {
        Some(
            resolve_agent(
                &state,
                user.id,
                &conversation,
                requested_model.as_deref(),
                requested_provider,
            )
            .await?,
        )
    } else {
        None
    };

    // Persist an assistant placeholder first so partial output survives a
    // client disconnect, then run generation in a detached task.
    let assistant = state
        .store
        .insert_message_with_status(
            conversation.id,
            "assistant",
            Some(""),
            None,
            None,
            message_status::STREAMING,
        )
        .await?;
    let message_id = assistant.id;

    let tx = state.hub.open(message_id);
    let rx = state.hub.subscribe(message_id).expect("just opened");

    match resolved {
        Some(resolved) => spawn_agent_generation(
            state.clone(),
            message_id,
            conversation.id,
            resolved,
            messages,
            tx,
        ),
        None => {
            let provider = match requested_provider {
                Some(provider_id) => resolve_provider(&state, provider_id, user.id).await?,
                None => default_provider(&state, user.id).await?,
            };
            let model = pick_model(provider.as_ref(), requested_model).await?;
            let request = ChatRequest {
                model,
                messages,
                temperature: None,
                max_tokens: None,
                tools: None,
            };
            spawn_generation(provider, state.clone(), message_id, request, tx);
        }
    }

    let meta = json!({
        "message_id": message_id,
        "conversation_id": conversation.id,
        "resume_path": format!("/api/messages/{message_id}/stream"),
    })
    .to_string();
    let sources_json = serde_json::to_string(&sources).unwrap_or_else(|_| "[]".into());

    let stream = async_stream::stream! {
        yield Ok(Event::default().event("meta").data(meta));
        if !sources.is_empty() {
            yield Ok(Event::default().event("sources").data(sources_json));
        }
        let live = live_stream(rx, 0);
        futures::pin_mut!(live);
        while let Some(event) = live.next().await {
            yield event;
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// `GET /api/messages/{id}/stream` — reconnect to a generation in progress, or
/// replay a finished one from `Last-Event-ID`.
pub async fn resume_stream(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(message_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let message = state
        .store
        .get_message(message_id)
        .await?
        .ok_or(ChatError::NotFound)?;
    state
        .store
        .get_conversation(message.conversation_id, user.id)
        .await?;
    if message.role != "assistant" {
        return Err(ChatError::BadRequest("only assistant messages can be resumed".into()).into());
    }

    let last_event_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let persisted = message.content.clone().unwrap_or_default();
    let status = message.status.clone();
    let rx = state.hub.subscribe(message_id);
    let store = state.store.clone();
    let conversation_id = message.conversation_id;
    let meta = json!({
        "message_id": message_id,
        "conversation_id": conversation_id,
        "resumed": true,
    })
    .to_string();

    let stream = async_stream::stream! {
        yield Ok(Event::default().event("meta").data(meta));
        let mut offset = last_event_id;

        if let Some(rx) = rx {
            // A live generation is available on this replica: stream from the
            // client's offset using full snapshots (no gaps).
            let live = live_stream(rx, offset);
            futures::pin_mut!(live);
            while let Some(event) = live.next().await {
                yield event;
            }
        } else {
            // No live generation: replay what was persisted.
            let (chunk, next) = diff(&persisted, offset);
            offset = next;
            if !chunk.is_empty() {
                yield Ok(Event::default().id(offset.to_string()).event("delta").data(chunk));
            }
            match status.as_str() {
                message_status::COMPLETE => {
                    yield Ok(Event::default().event("done").data(""));
                }
                message_status::STREAMING => {
                    // Generation is gone (e.g. restart / different replica).
                    let _ = store
                        .update_message_content(message_id, &persisted, message_status::INTERRUPTED)
                        .await;
                    yield Ok(Event::default().event("error").data("generation was interrupted"));
                    yield Ok(Event::default().event("done").data(""));
                }
                _ => {
                    yield Ok(Event::default().event("error").data("generation failed"));
                    yield Ok(Event::default().event("done").data(""));
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Spawn generation so it continues (and persists) even if the client
/// disconnects. Terminates the watch channel with a single terminal state.
fn spawn_generation(
    provider: Arc<dyn LlmProvider>,
    state: Arc<AppState>,
    message_id: Uuid,
    request: ChatRequest,
    tx: watch::Sender<StreamState>,
) {
    tokio::spawn(async move {
        let store = state.store.clone();
        let hub = state.hub.clone();
        match generate(provider.as_ref(), request, &tx, &store, message_id).await {
            Ok(content) => {
                if let Err(err) = store
                    .update_message_content(message_id, &content, message_status::COMPLETE)
                    .await
                {
                    tracing::error!(error = %err, "failed to persist assistant message");
                }
                let _ = tx.send(StreamState::Completed {
                    content,
                    events: Vec::new(),
                });
            }
            Err((partial, error)) => {
                if let Err(err) = store
                    .update_message_content(message_id, &partial, message_status::ERROR)
                    .await
                {
                    tracing::error!(error = %err, "failed to persist partial assistant message");
                }
                let _ = tx.send(StreamState::Failed {
                    content: partial,
                    error,
                    events: Vec::new(),
                });
            }
        }
        hub.close(message_id);
    });
}

/// Provider, config and tools resolved for a single chat turn.
struct ResolvedAgent {
    provider: Arc<dyn LlmProvider>,
    config: AgentConfig,
    tools: ToolRegistry,
}

/// Build the runtime provider/config/tools for a turn.
///
/// The provider is the agent's own when set, else the user's default (their
/// first provider, falling back to a global one). A plain conversation has no
/// agent; a conversation with an agent also derives its instructions, model,
/// and tool subset.
async fn resolve_agent(
    state: &AppState,
    user_id: Uuid,
    conversation: &Conversation,
    requested_model: Option<&str>,
    requested_provider: Option<Uuid>,
) -> Result<ResolvedAgent, ApiError> {
    let agent = match conversation.agent_id {
        Some(agent_id) => Some(
            state
                .store
                .get_agent(agent_id, user_id)
                .await?
                .ok_or(ChatError::NotFound)?,
        ),
        None => None,
    };

    // Provider: the explicitly requested one, else the agent's own, else the
    // user's default (own, then global).
    let provider = match requested_provider {
        Some(provider_id) => resolve_provider(state, provider_id, user_id).await?,
        None => match agent.as_ref().and_then(|agent| agent.provider_id) {
            Some(provider_id) => resolve_provider(state, provider_id, user_id).await?,
            None => default_provider(state, user_id).await?,
        },
    };

    let mut system = None;
    let mut model = requested_model.map(str::to_string);
    let mut tools = state.tools.clone();
    if let Some(agent) = &agent {
        if let Some(agent_model) = agent.model.clone().filter(|m| !m.trim().is_empty()) {
            model = Some(agent_model);
        }
        system = agent
            .instructions
            .clone()
            .filter(|instructions| !instructions.trim().is_empty());

        // The agent's tool list is authoritative; `sandbox_enabled` adds the
        // code interpreter on top when it is registered.
        let mut names = agent.tools.0.clone();
        if agent.sandbox_enabled && !names.iter().any(|name| name == "execute_code") {
            names.push("execute_code".to_string());
        }
        tools = state.tools.restricted(&names);
    }

    let model = pick_model(provider.as_ref(), model).await?;
    let config = AgentConfig {
        model,
        max_iterations: state.cfg.agent_max_iterations,
        system,
        temperature: None,
    };

    Ok(ResolvedAgent {
        provider,
        config,
        tools,
    })
}

/// Turn an agent's provider row into a usable client, falling back to the
/// user's default provider when the row no longer exists.
async fn resolve_provider(
    state: &AppState,
    provider_id: Uuid,
    user_id: Uuid,
) -> Result<Arc<dyn LlmProvider>, ApiError> {
    match state.store.get_provider(provider_id, user_id).await? {
        Some(row) => build_provider(state, &row, user_id).await,
        None => {
            tracing::warn!(%provider_id, "agent provider not found; using default");
            default_provider(state, user_id).await
        }
    }
}

/// Choose the model for a turn: the requested/agent model, else the provider's
/// first advertised model.
async fn pick_model(
    provider: &dyn LlmProvider,
    requested: Option<String>,
) -> Result<String, ApiError> {
    if let Some(model) = requested.filter(|m| !m.trim().is_empty()) {
        return Ok(model);
    }
    let models = provider
        .list_models()
        .await
        .map_err(|e| ChatError::Upstream(e.to_string()))?;
    models
        .into_iter()
        .next()
        .map(|model| model.id)
        .ok_or(ApiError(ChatError::ProviderNotConfigured))
}

/// Spawn the agent loop, forwarding tool turns to the client and persisting
/// them. Intermediate tool-call/tool-result messages are inserted between the
/// placeholder and the final answer; the placeholder is finalized (moved to the
/// end) once the loop stops.
fn spawn_agent_generation(
    state: Arc<AppState>,
    message_id: Uuid,
    conversation_id: Uuid,
    resolved: ResolvedAgent,
    messages: Vec<ChatMessage>,
    tx: watch::Sender<StreamState>,
) {
    tokio::spawn(async move {
        let store = state.store.clone();
        let hub = state.hub.clone();
        let mut events: Vec<SseEvent> = Vec::new();
        let mut accumulated = String::new();
        let mut last_persist = 0usize;
        let mut used_tools = false;
        let mut failure: Option<String> = None;

        let ResolvedAgent {
            provider,
            config,
            tools,
        } = resolved;
        let mut stream = run_agent_stream(provider, tools, config, messages);

        while let Some(item) = stream.next().await {
            match item {
                Ok(AgentEvent::Delta(text)) => {
                    accumulated.push_str(&text);
                    let _ = tx.send(StreamState::Streaming {
                        content: accumulated.clone(),
                        events: events.clone(),
                    });
                    if accumulated.len().saturating_sub(last_persist) >= PERSIST_EVERY_BYTES {
                        if let Err(err) = store
                            .update_message_content(
                                message_id,
                                &accumulated,
                                message_status::STREAMING,
                            )
                            .await
                        {
                            tracing::warn!(error = %err, "checkpoint persist failed");
                        }
                        last_persist = accumulated.len();
                    }
                }
                Ok(AgentEvent::ToolCalls {
                    content,
                    tool_calls,
                }) => {
                    used_tools = true;
                    if let Err(err) = store
                        .insert_message(
                            conversation_id,
                            "assistant",
                            Some(&content),
                            Some(tool_calls.clone()),
                            None,
                        )
                        .await
                    {
                        tracing::warn!(error = %err, "failed to persist tool-call message");
                    }
                    events.push(SseEvent::new("tool_call", tool_calls));
                    let _ = tx.send(StreamState::Streaming {
                        content: accumulated.clone(),
                        events: events.clone(),
                    });
                }
                Ok(AgentEvent::ToolResult { id, name, content }) => {
                    if let Err(err) = store
                        .insert_message(conversation_id, "tool", Some(&content), None, Some(&id))
                        .await
                    {
                        tracing::warn!(error = %err, "failed to persist tool result");
                    }
                    events.push(SseEvent::new(
                        "tool_result",
                        json!({ "tool_call_id": id, "name": name, "content": content }),
                    ));
                    let _ = tx.send(StreamState::Streaming {
                        content: accumulated.clone(),
                        events: events.clone(),
                    });
                }
                Err(err) => {
                    failure = Some(err.to_string());
                    break;
                }
            }
        }

        match failure {
            Some(error) => {
                if let Err(err) = store
                    .update_message_content(message_id, &accumulated, message_status::ERROR)
                    .await
                {
                    tracing::error!(error = %err, "failed to persist partial assistant message");
                }
                let _ = tx.send(StreamState::Failed {
                    content: accumulated,
                    error,
                    events,
                });
            }
            None => {
                // Tool turns were inserted after the placeholder, so move the
                // final answer to the end of the conversation to keep ordering.
                let result = if used_tools {
                    store
                        .finalize_message(message_id, &accumulated, message_status::COMPLETE)
                        .await
                } else {
                    store
                        .update_message_content(message_id, &accumulated, message_status::COMPLETE)
                        .await
                };
                if let Err(err) = result {
                    tracing::error!(error = %err, "failed to persist assistant message");
                }
                let _ = tx.send(StreamState::Completed {
                    content: accumulated,
                    events,
                });
            }
        }
        hub.close(message_id);
    });
}

/// Consume the provider stream, checkpointing and broadcasting as it goes.
/// Returns the full content on success, or `(partial_content, error)`.
async fn generate(
    provider: &dyn LlmProvider,
    request: ChatRequest,
    tx: &watch::Sender<StreamState>,
    store: &Arc<dyn Store>,
    message_id: Uuid,
) -> Result<String, (String, String)> {
    let mut upstream = match provider.stream(request).await {
        Ok(stream) => stream,
        Err(err) => return Err((String::new(), err.to_string())),
    };

    let mut accumulated = String::new();
    let mut last_persist = 0usize;

    while let Some(chunk) = upstream.next().await {
        match chunk {
            Ok(chunk) => {
                if !chunk.delta.is_empty() {
                    accumulated.push_str(&chunk.delta);
                    let _ = tx.send(StreamState::Streaming {
                        content: accumulated.clone(),
                        events: Vec::new(),
                    });
                    if accumulated.len().saturating_sub(last_persist) >= PERSIST_EVERY_BYTES {
                        if let Err(err) = store
                            .update_message_content(
                                message_id,
                                &accumulated,
                                message_status::STREAMING,
                            )
                            .await
                        {
                            tracing::warn!(error = %err, "checkpoint persist failed");
                        }
                        last_persist = accumulated.len();
                    }
                }
                if chunk.done {
                    break;
                }
            }
            Err(err) => return Err((accumulated, err.to_string())),
        }
    }
    Ok(accumulated)
}

/// Adapt a live generation into an SSE event stream, emitting only content the
/// client does not already have (below `offset`) and terminating on a terminal
/// state.
fn live_stream(
    mut rx: watch::Receiver<StreamState>,
    mut offset: usize,
) -> impl futures::Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        // Number of `tool_call`/`tool_result` events already forwarded.
        let mut emitted = 0usize;
        loop {
            let current = rx.borrow_and_update().clone();

            // Forward any auxiliary events (tool calls/results) that appeared
            // since the last checkpoint.
            let new_events = current.events()[emitted..].to_vec();
            emitted += new_events.len();
            for ev in new_events {
                yield Ok(Event::default().event(ev.name).data(ev.data.to_string()));
            }

            match current {
                StreamState::Streaming { content, .. } => {
                    let (chunk, next) = diff(&content, offset);
                    if !chunk.is_empty() {
                        offset = next;
                        yield Ok(Event::default().id(offset.to_string()).event("delta").data(chunk));
                    }
                    if rx.changed().await.is_err() {
                        // Producer vanished without a terminal state.
                        yield Ok(Event::default().event("done").data(""));
                        break;
                    }
                }
                StreamState::Completed { content, .. } => {
                    let (chunk, next) = diff(&content, offset);
                    if !chunk.is_empty() {
                        offset = next;
                        yield Ok(Event::default().id(offset.to_string()).event("delta").data(chunk));
                    }
                    yield Ok(Event::default().event("done").data(""));
                    break;
                }
                StreamState::Failed { content, error, .. } => {
                    let (chunk, next) = diff(&content, offset);
                    if !chunk.is_empty() {
                        offset = next;
                        yield Ok(Event::default().id(offset.to_string()).event("delta").data(chunk));
                    }
                    yield Ok(Event::default().event("error").data(error));
                    yield Ok(Event::default().event("done").data(""));
                    break;
                }
            }
        }
    }
}

/// Return the substring of `content` after `offset` bytes and the new offset.
/// `offset` is clamped to a UTF-8 char boundary.
fn diff(content: &str, offset: usize) -> (String, usize) {
    if offset >= content.len() {
        return (String::new(), content.len());
    }
    let mut start = offset;
    while start < content.len() && !content.is_char_boundary(start) {
        start += 1;
    }
    (content[start..].to_string(), content.len())
}

// ---- sandbox --------------------------------------------------------------

#[derive(Deserialize)]
pub struct SandboxBody {
    pub language: String,
    pub code: String,
    #[serde(default)]
    pub files: Vec<SandboxFile>,
}

pub async fn sandbox_run(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<SandboxBody>,
) -> Result<Json<ExecResult>, ApiError> {
    let language = body.language.clone();
    let request = ExecRequest {
        language: body.language,
        code: body.code,
        files: body.files,
    };
    let result = state
        .sandbox
        .run(&request)
        .await
        .map_err(|e| ApiError(ChatError::Internal(anyhow::anyhow!(e))))?;

    audit(
        &state,
        AuditEntry {
            actor_id: Some(user.id),
            action: "sandbox.exec".into(),
            target_type: Some("sandbox".into()),
            target_id: None,
            metadata: Some(json!({
                "language": language,
                "exit_code": result.exit_code,
                "timed_out": result.timed_out,
            })),
            ip: Some(addr.ip().to_string()),
        },
    )
    .await;

    Ok(Json(result))
}

/// Keep only characters that are safe in a file name.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        "upload".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::diff;

    #[test]
    fn diff_slices_from_offset() {
        assert_eq!(diff("hello", 2), ("llo".to_string(), 5));
        assert_eq!(diff("hello", 5), (String::new(), 5));
        // Offset past the end is clamped.
        assert_eq!(diff("hello", 99), (String::new(), 5));
    }

    #[test]
    fn diff_snaps_to_char_boundary() {
        // "héllo": 'h' is 1 byte, 'é' is 2 bytes, so byte 2 is mid-character.
        let (chunk, next) = diff("héllo", 2);
        assert_eq!(chunk, "llo");
        assert_eq!(next, "héllo".len());
    }
}
