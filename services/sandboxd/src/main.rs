//! `sandboxd` — a standalone sandbox execution service.
//!
//! Keeps the chat API process lean: the API can call this over HTTP instead of
//! holding sandbox state, and sandboxes can be scheduled on dedicated hosts.
//! Backend is selected with `SANDBOX_BACKEND=podman|boxlite`.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use chat_core::SandboxBackendKind;
use chat_sandbox::{
    BoxliteBackend, ExecRequest, ExecResult, PodmanBackend, SandboxBackend, SandboxSpec,
};

#[derive(Clone)]
struct AppState {
    backend: Arc<dyn SandboxBackend>,
    spec: SandboxSpec,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    let kind: SandboxBackendKind = std::env::var("SANDBOX_BACKEND")
        .unwrap_or_else(|_| "podman".into())
        .parse()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let backend: Arc<dyn SandboxBackend> = match kind {
        SandboxBackendKind::Podman => Arc::new(PodmanBackend::new()),
        SandboxBackendKind::Boxlite => Arc::new(BoxliteBackend::new(
            std::env::var("BOXLITE_URL").unwrap_or_else(|_| "http://localhost:8100".into()),
        )),
    };

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
        network: false,
    };

    let bind_addr = std::env::var("SANDBOXD_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3081".into());

    tracing::info!(backend = backend.name(), isolation = ?backend.isolation(), "sandboxd ready");

    let state = AppState { backend, spec };
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/exec", post(exec))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, "sandboxd listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn exec(
    State(state): State<AppState>,
    Json(request): Json<ExecRequest>,
) -> Result<Json<ExecResult>, (axum::http::StatusCode, String)> {
    state
        .backend
        .run(&state.spec, &request)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}
