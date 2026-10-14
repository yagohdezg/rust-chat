//! `sandboxd` — a standalone sandbox execution service.
//!
//! Keeps the chat API process lean: the API calls this over HTTP instead of
//! holding sandbox state, and sandboxes can be scheduled on dedicated hosts.
//! Execution always runs through BoxLite; the sandbox spec (image, limits,
//! timeout) is owned here, from the environment.
//!
//! `SANDBOXD_TOKEN` is mandatory: `/v1/exec` requires `Authorization: Bearer`
//! and `/health` is open.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use chat_sandbox::{
    BoxliteBackend, CommandSpec, ExecRequest, ExecResult, OutputChannel, SandboxBackend,
    SandboxSession, SandboxSpec,
};

#[derive(Clone)]
struct AppState {
    /// Concrete backend: computer endpoints need the persistent-box lifecycle,
    /// which is not part of the object-safe [`SandboxBackend`] surface.
    backend: Arc<BoxliteBackend>,
    token: Arc<String>,
}

/// Response wrapping a persistent box id.
#[derive(Serialize)]
struct ComputerRef {
    handle: String,
}

/// Optional body for provisioning a persistent box. `workspace` names a managed
/// volume to mount at the working directory so the workspace outlives the box;
/// `name` is a deployment-assigned box name used to mark ownership.
#[derive(Deserialize, Default)]
struct CreateComputerBody {
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// A box as reported by the node listing endpoint.
#[derive(Serialize)]
struct ComputerSummary {
    handle: String,
    name: Option<String>,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    let token = std::env::var("SANDBOXD_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("SANDBOXD_TOKEN is required (refusing to start without it)")
        })?;

    let boxlite_url =
        std::env::var("BOXLITE_URL").unwrap_or_else(|_| "http://localhost:8100".into());
    let boxlite_token = std::env::var("BOXLITE_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty());

    let network = std::env::var("SANDBOX_NETWORK")
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);
    let egress_allow = chat_sandbox::parse_egress_allow(
        &std::env::var("SANDBOX_EGRESS_ALLOW").unwrap_or_default(),
    );
    chat_sandbox::validate_egress_allow(&egress_allow)
        .map_err(|e| anyhow::anyhow!("invalid SANDBOX_EGRESS_ALLOW: {e}"))?;
    if !network && !egress_allow.is_empty() {
        return Err(anyhow::anyhow!(
            "SANDBOX_EGRESS_ALLOW is set but SANDBOX_NETWORK is false; \
             set SANDBOX_NETWORK=true to enable egress"
        ));
    }

    let spec = SandboxSpec {
        image: std::env::var("SANDBOX_IMAGE").unwrap_or_else(|_| "python:3.12-slim".into()),
        timeout_seconds: std::env::var("SANDBOX_TIMEOUT_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30),
        memory_mb: std::env::var("SANDBOX_MEMORY_MB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(512),
        cpus: std::env::var("SANDBOX_CPUS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0),
        disk_gb: std::env::var("SANDBOX_DISK_GB")
            .ok()
            .and_then(|v| v.parse().ok()),
        network,
        egress_allow,
    };

    let backend = Arc::new(BoxliteBackend::new(boxlite_url, boxlite_token, spec));

    let bind_addr = std::env::var("SANDBOXD_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3081".into());

    let egress = if !network {
        "denied"
    } else if backend.spec().egress_allow.is_empty() {
        "unrestricted"
    } else {
        "allow-list"
    };
    tracing::info!(
        backend = backend.name(),
        isolation = ?backend.isolation(),
        egress,
        allow_net = ?backend.spec().egress_allow,
        "sandboxd ready"
    );

    let state = AppState {
        backend,
        token: Arc::new(token),
    };
    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, "sandboxd listening");
    axum::serve(listener, app).await?;
    Ok(())
}

fn build_app(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/exec", post(exec))
        .route("/v1/exec/stream", get(exec_stream))
        .route("/v1/computers", post(create_computer).get(list_computers))
        .route("/v1/computers/{handle}/exec", post(exec_computer))
        .route("/v1/computers/{handle}/pause", post(pause_computer))
        .route("/v1/computers/{handle}/resume", post(resume_computer))
        .route("/v1/computers/{handle}", delete(destroy_computer))
        .route("/v1/workspaces/{name}", delete(destroy_workspace))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ));
    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

/// Reject requests without a valid `Authorization: Bearer <token>` header.
/// The comparison is constant-time so it does not leak the token via timing.
async fn require_bearer(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|token| {
            use subtle::ConstantTimeEq;
            token.as_bytes().ct_eq(state.token.as_bytes()).into()
        })
        .unwrap_or(false);

    if !authorized {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

async fn exec(
    State(state): State<AppState>,
    Json(request): Json<ExecRequest>,
) -> Result<Json<ExecResult>, (StatusCode, String)> {
    state
        .backend
        .run(&request)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Bridge a bidirectional WebSocket to a long-lived command in a fresh box.
///
/// The client's first frame is a JSON [`CommandSpec`]. Binary frames then flow
/// to the command's stdin, and the command's output flows back as
/// `[channel][payload]` binary frames. The box is torn down when either side
/// closes.
async fn exec_stream(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| proxy_session(socket, state))
}

async fn proxy_session(mut socket: WebSocket, state: AppState) {
    let command = match socket.recv().await {
        Some(Ok(Message::Text(text))) => match serde_json::from_str::<CommandSpec>(&text) {
            Ok(command) => command,
            Err(e) => {
                tracing::warn!(error = %e, "invalid exec stream command spec");
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
        },
        _ => return,
    };

    let session: Arc<dyn SandboxSession> = match state.backend.open_session(&command).await {
        Ok(session) => Arc::from(session),
        Err(e) => {
            tracing::warn!(error = %e, command = %command.command, "failed to open sandbox session");
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    let (mut tx, mut rx) = socket.split();

    let stdin_session = session.clone();
    let mut stdin_task = tokio::spawn(async move {
        while let Some(Ok(message)) = rx.next().await {
            match message {
                Message::Binary(bytes) => {
                    if stdin_session.write_stdin(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    loop {
        tokio::select! {
            chunk = session.read_output() => match chunk {
                Some(chunk) => {
                    let mut frame = Vec::with_capacity(chunk.data.len() + 1);
                    frame.push(match chunk.channel {
                        OutputChannel::Stderr => 0x02,
                        OutputChannel::Stdout => 0x01,
                    });
                    frame.extend_from_slice(&chunk.data);
                    if tx.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            _ = &mut stdin_task => break,
        }
    }

    stdin_task.abort();
    let _ = tx.send(Message::Close(None)).await;
    session.close().await;
}

/// Provision a persistent box for a per-user "computer", optionally mounting a
/// durable workspace volume.
async fn create_computer(
    State(state): State<AppState>,
    body: Option<Json<CreateComputerBody>>,
) -> Result<Json<ComputerRef>, (StatusCode, String)> {
    let body = body.map(|Json(body)| body).unwrap_or_default();
    state
        .backend
        .create_computer(body.workspace.as_deref(), body.name.as_deref())
        .await
        .map(|handle| Json(ComputerRef { handle }))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// List every box on this node, so the control plane can reconcile ownership.
async fn list_computers(
    State(state): State<AppState>,
) -> Result<Json<Vec<ComputerSummary>>, (StatusCode, String)> {
    state
        .backend
        .list_computers()
        .await
        .map(|boxes| {
            Json(
                boxes
                    .into_iter()
                    .map(|box_| ComputerSummary {
                        handle: box_.handle,
                        name: box_.name,
                        created_at: box_.created_at,
                    })
                    .collect(),
            )
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Run one request inside a persistent box, keeping the box alive.
async fn exec_computer(
    State(state): State<AppState>,
    Path(handle): Path<String>,
    Json(request): Json<ExecRequest>,
) -> Result<Json<ExecResult>, (StatusCode, String)> {
    state
        .backend
        .exec_computer(&handle, &request)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Pause a persistent box: stop the VM and snapshot its disks.
async fn pause_computer(
    State(state): State<AppState>,
    Path(handle): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .backend
        .pause_computer(&handle)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Resume a paused persistent box.
async fn resume_computer(
    State(state): State<AppState>,
    Path(handle): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .backend
        .resume_computer(&handle)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Destroy a persistent box. Idempotent.
async fn destroy_computer(
    State(state): State<AppState>,
    Path(handle): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .backend
        .destroy_computer(&handle)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Remove a durable workspace volume. Idempotent.
async fn destroy_workspace(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .backend
        .destroy_workspace(&name)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    fn test_state(token: &str) -> AppState {
        let backend = Arc::new(BoxliteBackend::new(
            "http://127.0.0.1:1",
            None,
            SandboxSpec::default(),
        ));
        AppState {
            backend,
            token: Arc::new(token.into()),
        }
    }

    fn exec_request(token: Option<&str>) -> HttpRequest<Body> {
        let builder = HttpRequest::builder()
            .method("POST")
            .uri("/v1/exec")
            .header("content-type", "application/json");
        let builder = match token {
            Some(token) => builder.header("authorization", format!("Bearer {token}")),
            None => builder,
        };
        builder.body(Body::from("{}")).unwrap()
    }

    #[tokio::test]
    async fn exec_without_token_is_unauthorized() {
        let response = build_app(test_state("right-token"))
            .oneshot(exec_request(None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn exec_with_wrong_token_is_unauthorized() {
        let response = build_app(test_state("right-token"))
            .oneshot(exec_request(Some("wrong-token")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_is_open() {
        let response = build_app(test_state("right-token"))
            .oneshot(
                HttpRequest::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[derive(Clone)]
    struct MockBoxlite {
        deletes: Arc<std::sync::atomic::AtomicUsize>,
    }

    async fn create_box() -> (StatusCode, Json<serde_json::Value>) {
        (
            StatusCode::CREATED,
            Json(serde_json::json!({ "box_id": "box-1" })),
        )
    }

    async fn start_exec(Path(_box_id): Path<String>) -> (StatusCode, Json<serde_json::Value>) {
        (
            StatusCode::CREATED,
            Json(serde_json::json!({ "execution_id": "exec-1" })),
        )
    }

    async fn attach(ws: WebSocketUpgrade) -> Response {
        ws.on_upgrade(|socket| async move {
            let (mut tx, mut rx) = socket.split();
            while let Some(Ok(message)) = rx.next().await {
                match message {
                    Message::Binary(bytes) => {
                        let mut frame = vec![0x01u8];
                        frame.extend_from_slice(&bytes);
                        if tx.send(Message::Binary(frame.into())).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            let _ = tx
                .send(Message::Text(r#"{"type":"exit","exit_code":0}"#.into()))
                .await;
            let _ = tx.send(Message::Close(None)).await;
        })
    }

    async fn delete_box(
        State(mock): State<MockBoxlite>,
        Path(_box_id): Path<String>,
    ) -> StatusCode {
        mock.deletes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        StatusCode::NO_CONTENT
    }

    async fn spawn_mock_boxlite() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let mock = MockBoxlite {
            deletes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/v1/boxes", post(create_box))
            .route("/v1/boxes/{box_id}/exec", post(start_exec))
            .route(
                "/v1/boxes/{box_id}/executions/{exec_id}/attach",
                get(attach),
            )
            .route("/v1/boxes/{box_id}", delete(delete_box))
            .with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), mock.deletes)
    }

    #[tokio::test]
    async fn exec_stream_bridges_stdin_and_stdout() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::Message as TMessage;

        let (boxlite_url, deletes) = spawn_mock_boxlite().await;
        let state = AppState {
            backend: Arc::new(BoxliteBackend::new(
                boxlite_url,
                None,
                SandboxSpec::default(),
            )),
            token: Arc::new("right-token".into()),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, build_app(state)).await;
        });

        let url = format!("ws://{addr}/v1/exec/stream");
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            axum::http::header::AUTHORIZATION,
            "Bearer right-token".parse().unwrap(),
        );
        let (ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let (mut tx, mut rx) = ws.split();

        let spec = serde_json::to_string(&CommandSpec {
            command: "echo-mcp".into(),
            ..Default::default()
        })
        .unwrap();
        tx.send(TMessage::Text(spec)).await.unwrap();
        tx.send(TMessage::Binary(b"hello\n".to_vec()))
            .await
            .unwrap();

        let mut got = Vec::new();
        while let Some(message) = rx.next().await {
            if let TMessage::Binary(bytes) = message.unwrap() {
                assert_eq!(bytes[0], 0x01, "expected a stdout frame");
                got.extend_from_slice(&bytes[1..]);
                if got.ends_with(b"\n") {
                    break;
                }
            }
        }
        assert_eq!(got, b"hello\n");

        tx.send(TMessage::Close(None)).await.unwrap();
        for _ in 0..100 {
            if deletes.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            deletes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the box was not torn down after the stream closed"
        );
    }
}
