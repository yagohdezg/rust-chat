use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};

const SESSION_ID: &str = "fake-mcp-session";

/// A minimal MCP Streamable HTTP server exposing a single `echo` tool.
///
/// Requests are answered as `text/event-stream` so the client's SSE parsing is
/// exercised. It hands out a session id on `initialize` and requires it back on
/// every later request, which checks that the client captures and echoes it.
pub struct FakeHttpMcp {
    pub url: String,
    _task: tokio::task::JoinHandle<()>,
}

impl FakeHttpMcp {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/mcp", post(handle));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            url: format!("http://{addr}/mcp"),
            _task: task,
        }
    }
}

async fn handle(headers: HeaderMap, body: String) -> Response {
    let message: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let method = message["method"].as_str().unwrap_or_default();
    let session = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok());
    if method != "initialize" && session != Some(SESSION_ID) {
        return (StatusCode::BAD_REQUEST, "missing mcp-session-id").into_response();
    }

    if message.get("id").is_none() {
        return Response::builder()
            .status(StatusCode::ACCEPTED)
            .header("mcp-session-id", SESSION_ID)
            .body(Body::empty())
            .unwrap();
    }

    let id = message["id"].clone();
    let reply = match method {
        "initialize" => json!({ "jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "echo-http", "version": "0" },
        }}),
        "tools/list" => json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [{
            "name": "echo",
            "description": "Echoes the given text",
            "inputSchema": {
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
            },
        }]}}),
        "tools/call" => {
            let text = message["params"]["arguments"]["text"]
                .as_str()
                .unwrap_or("");
            json!({ "jsonrpc": "2.0", "id": id, "result": {
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            }})
        }
        _ => json!({ "jsonrpc": "2.0", "id": id, "error": {
            "code": -32601,
            "message": "method not found",
        }}),
    };

    let payload = format!("event: message\ndata: {reply}\n\n");
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("mcp-session-id", SESSION_ID)
        .body(Body::from(payload))
        .unwrap()
}
