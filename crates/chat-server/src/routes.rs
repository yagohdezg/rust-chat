use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::watch;
use uuid::Uuid;

use chat_auth::{hash_password, issue_token, verify_password};
use chat_core::ChatError;
use chat_providers::{ChatMessage, ChatRequest, LlmProvider};
use chat_rag::RagPipeline;
use chat_sandbox::{ExecRequest, ExecResult, SandboxFile, SandboxSpec};
use chat_store::types::message_status;
use chat_store::{Conversation, FileRecord, Message, Scope, Store};

use crate::error::ApiError;
use crate::extract::AuthUser;
use crate::state::AppState;
use crate::stream::StreamState;

/// Persist a streaming reply to the database every this many bytes.
const PERSIST_EVERY_BYTES: usize = 256;

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
    Json(body): Json<RegisterBody>,
) -> Result<Json<Value>, ApiError> {
    if state.store.find_user_by_email(&body.email).await?.is_some() {
        return Err(ChatError::Conflict("email already registered".into()).into());
    }
    let hash = hash_password(&body.password)?;
    let user = state
        .store
        .create_user(&body.email, body.name.as_deref(), &hash, "user")
        .await?;
    let token = issue_token(
        &state.cfg.jwt_secret,
        user.id,
        &user.role,
        state.cfg.jwt_ttl_seconds,
    )?;
    Ok(Json(json!({
        "token": token,
        "user": { "id": user.id, "email": user.email, "name": user.name, "role": user.role }
    })))
}

#[derive(Deserialize)]
pub struct LoginBody {
    pub email: String,
    pub password: String,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginBody>,
) -> Result<Json<Value>, ApiError> {
    let user = state
        .store
        .find_user_by_email(&body.email)
        .await?
        .ok_or(ChatError::Unauthorized)?;
    let hash = user.password_hash.clone().ok_or(ChatError::Unauthorized)?;
    if !verify_password(&body.password, &hash)? {
        return Err(ChatError::Unauthorized.into());
    }
    let token = issue_token(
        &state.cfg.jwt_secret,
        user.id,
        &user.role,
        state.cfg.jwt_ttl_seconds,
    )?;
    Ok(Json(json!({
        "token": token,
        "user": { "id": user.id, "email": user.email, "name": user.name, "role": user.role }
    })))
}

pub async fn me(user: AuthUser) -> Json<Value> {
    Json(json!({ "id": user.id, "role": user.role }))
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
            ..Default::default()
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

    let request = ChatRequest {
        model: body.model.unwrap_or_else(|| "gpt-4o-mini".into()),
        messages,
        temperature: None,
        max_tokens: None,
        tools: None,
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
    spawn_generation(state.clone(), message_id, request, tx);

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
    state: Arc<AppState>,
    message_id: Uuid,
    request: ChatRequest,
    tx: watch::Sender<StreamState>,
) {
    tokio::spawn(async move {
        let store = state.store.clone();
        let hub = state.hub.clone();
        match generate(state.provider.as_ref(), request, &tx, &store, message_id).await {
            Ok(content) => {
                if let Err(err) = store
                    .update_message_content(message_id, &content, message_status::COMPLETE)
                    .await
                {
                    tracing::error!(error = %err, "failed to persist assistant message");
                }
                let _ = tx.send(StreamState::Completed { content });
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
        loop {
            let current = rx.borrow_and_update().clone();
            match current {
                StreamState::Streaming { content } => {
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
                StreamState::Completed { content } => {
                    let (chunk, next) = diff(&content, offset);
                    if !chunk.is_empty() {
                        offset = next;
                        yield Ok(Event::default().id(offset.to_string()).event("delta").data(chunk));
                    }
                    yield Ok(Event::default().event("done").data(""));
                    break;
                }
                StreamState::Failed { content, error } => {
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
    _user: AuthUser,
    Json(body): Json<SandboxBody>,
) -> Result<Json<ExecResult>, ApiError> {
    let spec = SandboxSpec {
        image: state.cfg.sandbox_image.clone(),
        timeout_seconds: state.cfg.sandbox_timeout_seconds,
        memory_mb: state.cfg.sandbox_memory_mb,
        cpus: state.cfg.sandbox_cpus,
        network: false,
    };
    let request = ExecRequest {
        language: body.language,
        code: body.code,
        files: body.files,
    };
    let result = state
        .sandbox
        .run(&spec, &request)
        .await
        .map_err(|e| ApiError(ChatError::Internal(anyhow::anyhow!(e))))?;
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
