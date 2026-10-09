use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::OnceCell;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::{
    program, ExecRequest, ExecResult, IsolationLevel, SandboxBackend, SandboxError, SandboxSpec,
};

/// Maximum bytes captured per stream (stdout / stderr) before clipping.
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Destination directory inside the box where the source and inputs land.
const WORK_DIR: &str = "/app";

/// BoxLite backend — one hardware-isolated microVM per run.
///
/// Talks to a running BoxLite server (`boxlite serve`, default
/// `http://localhost:8100`) rather than embedding the `boxlite` crate, so the
/// API process stays lean and sandboxes can run on dedicated hosts.
///
/// Lifecycle per run: `POST /boxes` -> `PUT /files` (in-memory tar) ->
/// `POST /exec` -> WebSocket attach -> `DELETE /boxes/{id}?force=true` on all
/// paths. Dual timeout: the execution carries `timeout_seconds`, and the whole
/// sequence is wrapped in a local wall-clock timeout. Output is capped at 1 MiB
/// per stream.
pub struct BoxliteBackend {
    base_url: String,
    token: Option<String>,
    spec: SandboxSpec,
    http: reqwest::Client,
    /// Cached routing prefix discovered from `GET /v1/me` (`path_prefix`).
    prefix: OnceCell<Option<String>>,
}

impl BoxliteBackend {
    pub fn new(base_url: impl Into<String>, token: Option<String>, spec: SandboxSpec) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.filter(|t| !t.trim().is_empty()),
            spec,
            http: reqwest::Client::new(),
            prefix: OnceCell::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn spec(&self) -> &SandboxSpec {
        &self.spec
    }

    fn endpoint(&self, prefix: &str, path: &str) -> String {
        if prefix.is_empty() {
            format!("{}/v1/{}", self.base_url, path)
        } else {
            format!("{}/v1/{}/{}", self.base_url, prefix, path)
        }
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// Resolve the optional routing prefix, caching it after the first lookup.
    ///
    /// Without a token the deployment is treated as single-tenant (`boxlite
    /// serve`) and no prefix is used.
    async fn prefix(&self) -> Result<&str, SandboxError> {
        if self.token.is_none() {
            return Ok("");
        }
        let prefix = self
            .prefix
            .get_or_try_init(|| async {
                let url = format!("{}/v1/me", self.base_url);
                let resp = self
                    .auth(self.http.get(url))
                    .send()
                    .await
                    .map_err(|e| transport("discover path prefix", e))?;
                let resp = ensure_success(resp, "identity lookup").await?;
                let principal: Principal = resp.json().await.map_err(|e| {
                    SandboxError::Transport(format!("invalid /v1/me response: {e}"))
                })?;
                Ok::<Option<String>, SandboxError>(
                    principal.path_prefix.filter(|p| !p.trim().is_empty()),
                )
            })
            .await?;
        Ok(prefix.as_deref().unwrap_or(""))
    }

    async fn create_box(&self, prefix: &str) -> Result<String, SandboxError> {
        let url = self.endpoint(prefix, "boxes");
        let body = serde_json::json!({
            "image": self.spec.image.clone(),
            "cpus": self.spec.cpus.round().max(1.0) as u32,
            "memory_mib": self.spec.memory_mb.max(128),
            "detach": false,
            "auto_delete": 0,
            "network": {
                "outbound": { "mode": if self.spec.network { "enabled" } else { "disabled" } }
            },
        });
        let resp = self
            .auth(self.http.post(url))
            .json(&body)
            .send()
            .await
            .map_err(|e| transport("create box", e))?;
        let resp = ensure_success(resp, "create box").await?;
        let created: BoxRef = resp
            .json()
            .await
            .map_err(|e| SandboxError::Transport(format!("invalid box response: {e}")))?;
        Ok(created.box_id)
    }

    async fn upload_files(
        &self,
        prefix: &str,
        box_id: &str,
        ext: &str,
        request: &ExecRequest,
    ) -> Result<(), SandboxError> {
        let archive = build_tar(request, ext)?;
        let url = self.endpoint(prefix, &format!("boxes/{box_id}/files"));
        let resp = self
            .auth(self.http.put(url))
            .query(&[("path", WORK_DIR)])
            .header(reqwest::header::CONTENT_TYPE, "application/x-tar")
            .body(archive)
            .send()
            .await
            .map_err(|e| transport("upload files", e))?;
        ensure_success(resp, "upload files").await?;
        Ok(())
    }

    async fn start_exec(
        &self,
        prefix: &str,
        box_id: &str,
        command: String,
        args: Vec<String>,
    ) -> Result<String, SandboxError> {
        let url = self.endpoint(prefix, &format!("boxes/{box_id}/exec"));
        let body = serde_json::json!({
            "command": command,
            "args": args,
            "timeout_seconds": self.spec.timeout_seconds.max(1) as f64,
            "working_dir": WORK_DIR,
        });
        let resp = self
            .auth(self.http.post(url))
            .json(&body)
            .send()
            .await
            .map_err(|e| transport("start execution", e))?;
        let resp = ensure_success(resp, "start execution").await?;
        let started: ExecRef = resp
            .json()
            .await
            .map_err(|e| SandboxError::Transport(format!("invalid exec response: {e}")))?;
        Ok(started.execution_id)
    }

    async fn kill_exec(
        &self,
        prefix: &str,
        box_id: &str,
        exec_id: &str,
    ) -> Result<(), SandboxError> {
        let url = self.endpoint(prefix, &format!("boxes/{box_id}/executions/{exec_id}"));
        let resp = self
            .auth(self.http.delete(url))
            .send()
            .await
            .map_err(|e| transport("kill execution", e))?;
        // A missing (already-finished) execution is fine.
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(SandboxError::Unavailable(format!(
                "kill execution failed: {}",
                resp.status()
            )))
        }
    }

    async fn delete_box(&self, prefix: &str, box_id: &str) -> Result<(), SandboxError> {
        let url = self.endpoint(prefix, &format!("boxes/{box_id}"));
        let resp = self
            .auth(self.http.delete(url))
            .query(&[("force", "true")])
            .send()
            .await
            .map_err(|e| transport("delete box", e))?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(SandboxError::Unavailable(format!(
                "delete box failed: {}",
                resp.status()
            )))
        }
    }

    fn ws_url(&self, prefix: &str, box_id: &str, exec_id: &str) -> String {
        let base = if let Some(rest) = self.base_url.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base_url.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.base_url.clone()
        };
        if prefix.is_empty() {
            format!("{base}/v1/boxes/{box_id}/executions/{exec_id}/attach")
        } else {
            format!("{base}/v1/{prefix}/boxes/{box_id}/executions/{exec_id}/attach")
        }
    }

    async fn stream_exec(
        &self,
        prefix: &str,
        box_id: &str,
        exec_id: &str,
    ) -> Result<ExecResult, SandboxError> {
        let ws_url = self.ws_url(prefix, box_id, exec_id);
        let mut request = ws_url
            .clone()
            .into_client_request()
            .map_err(|e| SandboxError::Transport(format!("invalid attach URL {ws_url}: {e}")))?;
        if let Some(token) = &self.token {
            let value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| SandboxError::Transport(format!("invalid token: {e}")))?;
            request.headers_mut().insert(AUTHORIZATION, value);
        }

        let (ws, _resp) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| SandboxError::Transport(format!("attach failed: {e}")))?;
        let (mut _sink, mut stream) = ws.split();

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut truncated = false;
        let mut exit_code: i32 = -1;

        while let Some(message) = stream.next().await {
            match message.map_err(|e| SandboxError::Transport(format!("attach stream: {e}")))? {
                Message::Binary(bytes) => {
                    if bytes.is_empty() {
                        continue;
                    }
                    let (channel, payload) = bytes.split_at(1);
                    match channel[0] {
                        0x01 => push_capped(&mut stdout, payload, &mut truncated),
                        0x02 => push_capped(&mut stderr, payload, &mut truncated),
                        _ => {
                            tracing::debug!(channel = channel[0], "ignoring unknown BoxLite frame")
                        }
                    }
                }
                Message::Text(text) => {
                    if let Ok(control) = serde_json::from_str::<ControlFrame>(&text) {
                        if control.kind == "exit" {
                            exit_code = control.exit_code.unwrap_or(-1);
                            break;
                        }
                    }
                }
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }

        // Keep the connection from being dropped before the close frame is read.
        let _ = _sink.close().await;

        Ok(ExecResult {
            exit_code,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            timed_out: false,
            truncated,
        })
    }

    async fn run_in_box(
        &self,
        prefix: &str,
        box_id: &str,
        program: (String, Vec<String>, &'static str),
        request: &ExecRequest,
        exec_id_cell: Arc<Mutex<Option<String>>>,
    ) -> Result<ExecResult, SandboxError> {
        let (command, args, ext) = program;
        self.upload_files(prefix, box_id, ext, request).await?;
        let exec_id = self.start_exec(prefix, box_id, command, args).await?;
        *exec_id_cell.lock().unwrap() = Some(exec_id.clone());
        self.stream_exec(prefix, box_id, &exec_id).await
    }
}

#[async_trait::async_trait]
impl SandboxBackend for BoxliteBackend {
    fn name(&self) -> &str {
        "boxlite"
    }

    fn isolation(&self) -> IsolationLevel {
        IsolationLevel::MicroVm
    }

    async fn run(&self, request: &ExecRequest) -> Result<ExecResult, SandboxError> {
        let prefix = self.prefix().await?.to_owned();
        let program = program(&request.language)
            .ok_or_else(|| SandboxError::UnsupportedLanguage(request.language.clone()))?;

        let box_id = self.create_box(&prefix).await?;

        let exec_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let deadline = Duration::from_secs(self.spec.timeout_seconds.max(1));
        let inner = self.run_in_box(&prefix, &box_id, program, request, exec_id.clone());
        let result = match tokio::time::timeout(deadline, inner).await {
            Ok(result) => result,
            Err(_elapsed) => {
                tracing::warn!(
                    box_id,
                    "BoxLite run exceeded local timeout; killing execution"
                );
                let timed_out_exec = exec_id.lock().unwrap().clone();
                if let Some(exec_id) = timed_out_exec {
                    if let Err(e) = self.kill_exec(&prefix, &box_id, &exec_id).await {
                        tracing::warn!(error = %e, "failed to kill timed-out execution");
                    }
                }
                Ok(ExecResult {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: String::new(),
                    timed_out: true,
                    truncated: false,
                })
            }
        };

        // Always remove the box, including on timeout and error paths.
        if let Err(e) = self.delete_box(&prefix, &box_id).await {
            tracing::warn!(error = %e, box_id, "failed to delete BoxLite box");
        }

        result
    }
}

#[derive(Debug, Deserialize)]
struct Principal {
    #[serde(default)]
    path_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BoxRef {
    box_id: String,
}

#[derive(Debug, Deserialize)]
struct ExecRef {
    execution_id: String,
}

#[derive(Debug, Deserialize)]
struct ControlFrame {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    exit_code: Option<i32>,
}

fn push_capped(buffer: &mut Vec<u8>, incoming: &[u8], truncated: &mut bool) {
    if buffer.len() >= MAX_OUTPUT_BYTES {
        *truncated = true;
        return;
    }
    let remaining = MAX_OUTPUT_BYTES - buffer.len();
    if incoming.len() > remaining {
        buffer.extend_from_slice(&incoming[..remaining]);
        *truncated = true;
    } else {
        buffer.extend_from_slice(incoming);
    }
}

fn build_tar(request: &ExecRequest, ext: &str) -> Result<Vec<u8>, SandboxError> {
    let mut builder = tar::Builder::new(Vec::new());
    append_entry(
        &mut builder,
        &format!("main.{ext}"),
        request.code.as_bytes(),
    )?;
    for file in &request.files {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&file.content_b64)
            .map_err(|e| SandboxError::Other(format!("invalid base64 in {}: {e}", file.name)))?;
        append_entry(&mut builder, &file.name, &bytes)?;
    }
    builder
        .into_inner()
        .map_err(|e| SandboxError::Other(format!("tar error: {e}")))
}

fn append_entry<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    data: &[u8],
) -> Result<(), SandboxError> {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, data)
        .map_err(|e| SandboxError::Other(format!("tar error: {e}")))
}

fn transport(action: &str, error: reqwest::Error) -> SandboxError {
    SandboxError::Transport(format!("{action}: {error}"))
}

async fn ensure_success(
    resp: reqwest::Response,
    what: &str,
) -> Result<reqwest::Response, SandboxError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    Err(SandboxError::Unavailable(format!(
        "BoxLite {what} failed: {status} {body}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ws::{Message as WsMessage, WebSocketUpgrade};
    use axum::extract::Path;
    use axum::routing::{delete, get, post, put};
    use axum::{Json, Router};
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct Recording {
        calls: StdMutex<Vec<String>>,
    }

    impl Recording {
        fn record(&self, method: &str, uri: &str) {
            self.calls.lock().unwrap().push(format!("{method} {uri}"));
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    type Params = std::collections::HashMap<String, String>;

    async fn spawn_mock(prefix: &'static str, slow_attach: bool) -> (String, Arc<Recording>) {
        let recording = Arc::new(Recording::default());

        let me = {
            let recording = recording.clone();
            move |uri: axum::http::Uri| {
                let recording = recording.clone();
                async move {
                    recording.record("GET", uri.path());
                    Json(serde_json::json!({
                        "sub": "svc",
                        "principal_type": "service_account",
                        "scopes": [],
                        "path_prefix": prefix,
                    }))
                }
            }
        };

        let create = {
            let recording = recording.clone();
            move |uri: axum::http::Uri| {
                let recording = recording.clone();
                async move {
                    recording.record("POST", uri.path());
                    (
                        axum::http::StatusCode::CREATED,
                        Json(serde_json::json!({ "box_id": "box-1" })),
                    )
                }
            }
        };

        let files = {
            let recording = recording.clone();
            move |Path(params): Path<Params>, uri: axum::http::Uri| {
                let recording = recording.clone();
                async move {
                    recording.record("PUT", uri.path());
                    assert_eq!(params.get("box_id").map(String::as_str), Some("box-1"));
                    axum::http::StatusCode::NO_CONTENT
                }
            }
        };

        let exec = {
            let recording = recording.clone();
            move |Path(params): Path<Params>, uri: axum::http::Uri| {
                let recording = recording.clone();
                async move {
                    recording.record("POST", uri.path());
                    assert_eq!(params.get("box_id").map(String::as_str), Some("box-1"));
                    (
                        axum::http::StatusCode::CREATED,
                        Json(serde_json::json!({ "execution_id": "exec-1" })),
                    )
                }
            }
        };

        let attach = {
            let recording = recording.clone();
            move |Path(params): Path<Params>, ws: WebSocketUpgrade, uri: axum::http::Uri| {
                let recording = recording.clone();
                async move {
                    recording.record("GET", uri.path());
                    assert_eq!(params.get("exec_id").map(String::as_str), Some("exec-1"));
                    ws.on_upgrade(move |socket| async move {
                        if slow_attach {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            return;
                        }
                        let (mut sender, _receiver) = socket.split();
                        let _ = sender
                            .send(WsMessage::Binary(vec![0x01, b'h', b'i'].into()))
                            .await;
                        let _ = sender
                            .send(WsMessage::Binary(vec![0x02, b'e', b'r', b'r'].into()))
                            .await;
                        let _ = sender
                            .send(WsMessage::Text(r#"{"type":"exit","exit_code":0}"#.into()))
                            .await;
                        let _ = sender.close().await;
                    })
                }
            }
        };

        let remove = {
            let recording = recording.clone();
            move |Path(params): Path<Params>, uri: axum::http::Uri| {
                let recording = recording.clone();
                async move {
                    recording.record("DELETE", uri.path());
                    assert_eq!(params.get("box_id").map(String::as_str), Some("box-1"));
                    axum::http::StatusCode::NO_CONTENT
                }
            }
        };

        let kill = {
            let recording = recording.clone();
            move |Path(params): Path<Params>, uri: axum::http::Uri| {
                let recording = recording.clone();
                async move {
                    recording.record("DELETE", uri.path());
                    assert_eq!(params.get("exec_id").map(String::as_str), Some("exec-1"));
                    axum::http::StatusCode::NO_CONTENT
                }
            }
        };

        let base = if prefix.is_empty() {
            Router::new()
                .route("/v1/me", get(me))
                .route("/v1/boxes", post(create))
                .route("/v1/boxes/{box_id}/files", put(files))
                .route("/v1/boxes/{box_id}/exec", post(exec))
                .route(
                    "/v1/boxes/{box_id}/executions/{exec_id}/attach",
                    get(attach),
                )
                .route("/v1/boxes/{box_id}/executions/{exec_id}", delete(kill))
                .route("/v1/boxes/{box_id}", delete(remove))
        } else {
            Router::new()
                .route("/v1/me", get(me))
                .route("/v1/{prefix}/boxes", post(create))
                .route("/v1/{prefix}/boxes/{box_id}/files", put(files))
                .route("/v1/{prefix}/boxes/{box_id}/exec", post(exec))
                .route(
                    "/v1/{prefix}/boxes/{box_id}/executions/{exec_id}/attach",
                    get(attach),
                )
                .route(
                    "/v1/{prefix}/boxes/{box_id}/executions/{exec_id}",
                    delete(kill),
                )
                .route("/v1/{prefix}/boxes/{box_id}", delete(remove))
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, base).await.unwrap();
        });
        (format!("http://{addr}"), recording)
    }

    fn request() -> ExecRequest {
        ExecRequest {
            language: "python".into(),
            code: "print('hi')".into(),
            files: vec![crate::SandboxFile {
                name: "data.txt".into(),
                content_b64: base64::engine::general_purpose::STANDARD.encode(b"payload"),
            }],
        }
    }

    #[test]
    fn multi_segment_prefix_is_substituted_verbatim() {
        let backend = BoxliteBackend::new(
            "http://host:8100",
            Some("tok".into()),
            SandboxSpec::default(),
        );
        assert_eq!(
            backend.endpoint("us-east/team-42", "boxes"),
            "http://host:8100/v1/us-east/team-42/boxes"
        );
        assert_eq!(
            backend.ws_url("us-east/team-42", "box-1", "exec-1"),
            "ws://host:8100/v1/us-east/team-42/boxes/box-1/executions/exec-1/attach"
        );
        assert_eq!(backend.endpoint("", "boxes"), "http://host:8100/v1/boxes");
    }

    #[tokio::test]
    async fn create_exec_attach_delete_roundtrip() {
        let (url, recording) = spawn_mock("team", false).await;
        let backend = BoxliteBackend::new(
            url,
            Some("secret".into()),
            SandboxSpec {
                timeout_seconds: 5,
                ..SandboxSpec::default()
            },
        );

        let result = backend.run(&request()).await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hi");
        assert_eq!(result.stderr, "err");
        assert!(!result.timed_out);

        let calls = recording.calls();
        assert!(calls.iter().any(|c| c == "GET /v1/me"), "{calls:?}");
        assert!(
            calls.iter().any(|c| c == "POST /v1/team/boxes"),
            "{calls:?}"
        );
        assert!(
            calls.iter().any(|c| c == "PUT /v1/team/boxes/box-1/files"),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c == "GET /v1/team/boxes/box-1/executions/exec-1/attach"),
            "{calls:?}"
        );
        assert!(
            calls.iter().any(|c| c == "DELETE /v1/team/boxes/box-1"),
            "{calls:?}"
        );
    }

    #[tokio::test]
    async fn without_token_no_prefix_discovery() {
        let (url, recording) = spawn_mock("", false).await;
        let backend = BoxliteBackend::new(
            url,
            None,
            SandboxSpec {
                timeout_seconds: 5,
                ..SandboxSpec::default()
            },
        );

        let result = backend.run(&request()).await.unwrap();
        assert_eq!(result.stdout, "hi");

        let calls = recording.calls();
        assert!(
            !calls.iter().any(|c| c.starts_with("GET /v1/me")),
            "{calls:?}"
        );
        assert!(calls.iter().any(|c| c == "POST /v1/boxes"), "{calls:?}");
    }

    #[tokio::test]
    async fn local_timeout_kills_and_cleans_up() {
        let (url, recording) = spawn_mock("org", true).await;
        let backend = BoxliteBackend::new(
            url,
            Some("secret".into()),
            SandboxSpec {
                timeout_seconds: 1,
                ..SandboxSpec::default()
            },
        );

        let result = backend.run(&request()).await.unwrap();
        assert!(result.timed_out);
        assert_eq!(result.exit_code, -1);

        let calls = recording.calls();
        assert!(
            calls
                .iter()
                .any(|c| c == "DELETE /v1/org/boxes/box-1/executions/exec-1"),
            "{calls:?}"
        );
        assert!(
            calls.iter().any(|c| c == "DELETE /v1/org/boxes/box-1"),
            "{calls:?}"
        );
    }
}
