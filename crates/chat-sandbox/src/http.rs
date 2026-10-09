use crate::{ExecRequest, ExecResult, IsolationLevel, SandboxBackend, SandboxError};

/// HTTP client for a standalone `sandboxd` service.
///
/// `chat-server` always talks to sandbox execution through this backend; the
/// real isolation level is decided by the remote service (BoxLite today), so
/// [`IsolationLevel::Remote`] is reported here.
pub struct HttpSandboxBackend {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl HttpSandboxBackend {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.filter(|t| !t.trim().is_empty()),
            http: reqwest::Client::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[async_trait::async_trait]
impl SandboxBackend for HttpSandboxBackend {
    fn name(&self) -> &str {
        "http"
    }

    fn isolation(&self) -> IsolationLevel {
        IsolationLevel::Remote
    }

    async fn run(&self, request: &ExecRequest) -> Result<ExecResult, SandboxError> {
        let url = format!("{}/v1/exec", self.base_url);
        let mut builder = self.http.post(url).json(request);
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| SandboxError::Transport(format!("sandboxd request failed: {e}")))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(SandboxError::Unavailable(
                "sandboxd rejected the bearer token".into(),
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SandboxError::Unavailable(format!(
                "sandboxd returned {status}: {body}"
            )));
        }

        resp.json::<ExecResult>()
            .await
            .map_err(|e| SandboxError::Transport(format!("invalid sandboxd response: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<Option<String>>>>;

    async fn mock_exec(
        State(seen): State<Seen>,
        headers: HeaderMap,
        Json(_request): Json<ExecRequest>,
    ) -> (StatusCode, Json<ExecResult>) {
        let auth = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        seen.lock().unwrap().push(auth);
        (
            StatusCode::OK,
            Json(ExecResult {
                exit_code: 0,
                stdout: "ok".into(),
                stderr: String::new(),
                timed_out: false,
                truncated: false,
            }),
        )
    }

    #[tokio::test]
    async fn posts_request_with_bearer_token() {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/exec", post(mock_exec))
            .with_state(seen.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let backend = HttpSandboxBackend::new(format!("http://{addr}"), Some("s3cret".into()));
        assert_eq!(backend.isolation(), IsolationLevel::Remote);

        let result = backend
            .run(&ExecRequest {
                language: "python".into(),
                code: "print(1)".into(),
                files: Vec::new(),
            })
            .await
            .unwrap();

        assert_eq!(result.stdout, "ok");
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[Some("Bearer s3cret".to_string())]
        );
    }

    #[tokio::test]
    async fn maps_unauthorized_to_unavailable() {
        async fn unauthorized() -> StatusCode {
            StatusCode::UNAUTHORIZED
        }
        let app = Router::new().route("/v1/exec", post(unauthorized));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let backend = HttpSandboxBackend::new(format!("http://{addr}"), Some("nope".into()));
        let error = backend
            .run(&ExecRequest {
                language: "python".into(),
                code: "print(1)".into(),
                files: Vec::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(error, SandboxError::Unavailable(_)));
    }
}
