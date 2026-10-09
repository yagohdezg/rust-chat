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

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use chat_sandbox::{BoxliteBackend, ExecRequest, ExecResult, SandboxBackend, SandboxSpec};

#[derive(Clone)]
struct AppState {
    backend: Arc<dyn SandboxBackend>,
    token: Arc<String>,
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

    let backend: Arc<dyn SandboxBackend> =
        Arc::new(BoxliteBackend::new(boxlite_url, boxlite_token, spec));

    let bind_addr = std::env::var("SANDBOXD_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3081".into());

    tracing::info!(backend = backend.name(), isolation = ?backend.isolation(), "sandboxd ready");

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
    let protected =
        Router::new()
            .route("/v1/exec", post(exec))
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    fn test_state(token: &str) -> AppState {
        let backend: Arc<dyn SandboxBackend> = Arc::new(BoxliteBackend::new(
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
}
