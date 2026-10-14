use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chat_sandbox::ExecResult;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;

#[derive(Clone)]
pub struct FakeSandbox {
    replies: Arc<Mutex<VecDeque<ExecResult>>>,
    fallback: Arc<Mutex<ExecResult>>,
    forced_status: Arc<Mutex<Option<u16>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Default for FakeSandbox {
    fn default() -> Self {
        Self {
            replies: Arc::new(Mutex::new(VecDeque::new())),
            fallback: Arc::new(Mutex::new(ExecResult {
                exit_code: 0,
                stdout: "ok\n".into(),
                ..Default::default()
            })),
            forced_status: Arc::new(Mutex::new(None)),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl FakeSandbox {
    pub async fn start() -> (Self, String) {
        let sandbox = Self::default();
        let app = Router::new()
            .route("/v1/exec", post(exec))
            .route("/v1/exec/stream", get(exec_stream))
            .with_state(sandbox.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (sandbox, format!("http://{addr}"))
    }

    pub fn push_reply(&self, reply: ExecResult) {
        self.replies.lock().unwrap().push_back(reply);
    }

    pub fn set_fallback(&self, reply: ExecResult) {
        *self.fallback.lock().unwrap() = reply;
    }

    pub fn set_forced_status(&self, status: Option<u16>) {
        *self.forced_status.lock().unwrap() = status;
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

/// Streaming exec: the test double for a sandboxed stdio process. It answers
/// MCP JSON-RPC (an `echo` tool) over the `[channel][payload]` frame protocol,
/// so the whole `chat-server -> sandboxd -> box` bridge is exercised.
async fn exec_stream(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle_stream)
}

async fn handle_stream(mut socket: WebSocket) {
    match socket.recv().await {
        Some(Ok(Message::Text(_))) => {}
        _ => return,
    }

    let mut stdin = Vec::new();
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            Message::Binary(bytes) => {
                stdin.extend_from_slice(&bytes);
                while let Some(pos) = stdin.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = stdin.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&line);
                    if let Some(reply) = echo_reply(line.trim()) {
                        let mut frame = vec![0x01u8];
                        frame.extend_from_slice(reply.as_bytes());
                        frame.push(b'\n');
                        if socket.send(Message::Binary(frame.into())).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    let _ = socket
        .send(Message::Text(r#"{"type":"exit","exit_code":0}"#.into()))
        .await;
    let _ = socket.send(Message::Close(None)).await;
}

/// Answer one MCP request line, or `None` for notifications/other traffic.
fn echo_reply(line: &str) -> Option<String> {
    let msg: Value = serde_json::from_str(line).ok()?;
    let id = msg.get("id")?.clone();
    let message = match msg.get("method").and_then(Value::as_str)? {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "echo-sandbox", "version": "0" },
            },
        }),
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [{
                    "name": "echo",
                    "description": "Echoes the given text",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "text": { "type": "string" } },
                        "required": ["text"],
                    },
                }],
            },
        }),
        "tools/call" => {
            let text = msg
                .get("params")
                .and_then(|params| params.get("arguments"))
                .and_then(|arguments| arguments.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{ "type": "text", "text": text }], "isError": false },
            })
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "method not found" },
        }),
    };
    Some(message.to_string())
}

async fn exec(State(s): State<FakeSandbox>, Json(body): Json<Value>) -> Response {
    s.requests.lock().unwrap().push(body);
    if let Some(code) = *s.forced_status.lock().unwrap() {
        return StatusCode::from_u16(code).unwrap().into_response();
    }
    let reply = s
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| s.fallback.lock().unwrap().clone());
    Json(reply).into_response()
}
