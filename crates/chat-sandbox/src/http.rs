use std::sync::atomic::{AtomicBool, Ordering};

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::{
    CommandSpec, ExecRequest, ExecResult, IsolationLevel, OutputChannel, OutputChunk,
    SandboxBackend, SandboxError, SandboxSession,
};

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

    async fn open_session(
        &self,
        command: &CommandSpec,
    ) -> Result<Box<dyn SandboxSession>, SandboxError> {
        let ws_url = format!("{}/v1/exec/stream", ws_base(&self.base_url));
        let mut request = ws_url
            .clone()
            .into_client_request()
            .map_err(|e| SandboxError::Transport(format!("invalid exec URL {ws_url}: {e}")))?;
        if let Some(token) = &self.token {
            let value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| SandboxError::Transport(format!("invalid token: {e}")))?;
            request.headers_mut().insert(AUTHORIZATION, value);
        }

        let (ws, _resp) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| SandboxError::Transport(format!("exec stream failed: {e}")))?;
        let (mut sink, stream) = ws.split();

        let spec = serde_json::to_string(command)
            .map_err(|e| SandboxError::Other(format!("invalid command spec: {e}")))?;
        sink.send(Message::Text(spec))
            .await
            .map_err(|e| SandboxError::Transport(format!("exec stream failed: {e}")))?;

        Ok(Box::new(HttpSession {
            sink: tokio::sync::Mutex::new(Some(sink)),
            stream: tokio::sync::Mutex::new(Some(stream)),
            closed: AtomicBool::new(false),
        }))
    }
}

/// Derive the `ws`/`wss` base for a sandboxd HTTP base URL.
fn ws_base(base_url: &str) -> String {
    if let Some(rest) = base_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base_url.to_string()
    }
}

/// A live session proxied through `sandboxd`'s streaming exec endpoint.
struct HttpSession {
    sink: tokio::sync::Mutex<Option<HttpSink>>,
    stream: tokio::sync::Mutex<Option<HttpStream>>,
    closed: AtomicBool,
}

#[async_trait::async_trait]
impl SandboxSession for HttpSession {
    async fn write_stdin(&self, data: &[u8]) -> Result<(), SandboxError> {
        let mut guard = self.sink.lock().await;
        let sink = guard
            .as_mut()
            .ok_or_else(|| SandboxError::Other("session is closed".into()))?;
        sink.send(Message::Binary(data.to_vec()))
            .await
            .map_err(|e| SandboxError::Transport(format!("stdin write failed: {e}")))
    }

    async fn read_output(&self) -> Option<OutputChunk> {
        let mut guard = self.stream.lock().await;
        let stream = guard.as_mut()?;
        loop {
            match stream.next().await {
                Some(Ok(Message::Binary(bytes))) if !bytes.is_empty() => {
                    let (channel, payload) = bytes.split_at(1);
                    let channel = if channel[0] == 0x02 {
                        OutputChannel::Stderr
                    } else {
                        OutputChannel::Stdout
                    };
                    return Some(OutputChunk {
                        channel,
                        data: payload.to_vec(),
                    });
                }
                Some(Ok(Message::Text(text))) => {
                    if let Ok(control) = serde_json::from_str::<serde_json::Value>(&text) {
                        if control.get("type").and_then(|v| v.as_str()) == Some("exit") {
                            return None;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
                Some(Ok(_)) => continue,
            }
        }
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(sink) = self.sink.lock().await.as_mut() {
            let _ = sink.send(Message::Close(None)).await;
            let _ = sink.close().await;
        }
    }
}

impl Drop for HttpSession {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.sink.try_lock() {
            if let Some(mut sink) = guard.take() {
                tokio::spawn(async move {
                    let _ = sink.send(Message::Close(None)).await;
                    let _ = sink.close().await;
                });
            }
        }
    }
}

type HttpSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type HttpStream = futures::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

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
                files: Vec::new(),
                rejected_files: Vec::new(),
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
                outputs: Vec::new(),
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
                outputs: Vec::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(error, SandboxError::Unavailable(_)));
    }
}
