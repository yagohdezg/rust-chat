//! Model Context Protocol (MCP) client.
//!
//! Implements JSON-RPC 2.0 over two transports:
//! - `stdio`: spawns a server process and exchanges newline-delimited messages.
//! - `http`: Streamable HTTP — POSTs JSON-RPC and reads a JSON or SSE response.
//!
//! Both clients accept concurrent requests: each is correlated to its response
//! by JSON-RPC id, so one slow call never blocks the others.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server returned error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("protocol error: {0}")]
    Protocol(String),
}

/// MCP transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    Http,
}

impl Transport {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stdio" => Some(Self::Stdio),
            "http" => Some(Self::Http),
            _ => None,
        }
    }
}

/// Everything needed to (re)connect an MCP server, independent of transport.
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: Transport,
    pub command: Option<String>,
    pub url: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// A connected MCP server over either transport, with a uniform interface.
pub enum McpClient {
    Stdio(Box<StdioMcpClient>),
    Http(HttpMcpClient),
}

impl McpClient {
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        match config.transport {
            Transport::Stdio => {
                let command = config.command.as_deref().ok_or_else(|| {
                    McpError::Protocol(format!("stdio server `{}` has no command", config.name))
                })?;
                Ok(Self::Stdio(Box::new(
                    StdioMcpClient::spawn(command, &config.args, &config.env).await?,
                )))
            }
            Transport::Http => {
                let url = config.url.as_deref().ok_or_else(|| {
                    McpError::Protocol(format!("http server `{}` has no url", config.name))
                })?;
                Ok(Self::Http(HttpMcpClient::connect(url)?))
            }
        }
    }

    /// Whether the connection is still usable. HTTP has no process to watch, so
    /// it always reports `true` and lets a failed request surface the error.
    pub fn is_running(&self) -> bool {
        match self {
            Self::Stdio(client) => client.is_running(),
            Self::Http(client) => client.is_running(),
        }
    }

    pub async fn initialize(&self) -> Result<Value, McpError> {
        match self {
            Self::Stdio(client) => client.initialize().await,
            Self::Http(client) => client.initialize().await,
        }
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        match self {
            Self::Stdio(client) => client.list_tools().await,
            Self::Http(client) => client.list_tools().await,
        }
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        match self {
            Self::Stdio(client) => client.call_tool(name, arguments).await,
            Self::Http(client) => client.call_tool(name, arguments).await,
        }
    }
}

/// The `initialize` request params shared by both transports.
fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "rust-chat", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// Extract the tool list from a `tools/list` result.
fn tools_from(result: Value) -> Result<Vec<McpTool>, McpError> {
    let tools = result
        .get("tools")
        .cloned()
        .unwrap_or_else(|| Value::Array(vec![]));
    serde_json::from_value(tools).map_err(|e| McpError::Protocol(e.to_string()))
}

/// Interpret a JSON-RPC message for request `id`. Returns `Ok(None)` when the
/// message is a notification or a response to a different request.
fn parse_message(value: Value, id: i64) -> Result<Option<Value>, McpError> {
    if value.get("id").and_then(Value::as_i64) != Some(id) {
        return Ok(None);
    }
    if let Some(err) = value.get("error") {
        return Err(McpError::Rpc {
            code: err.get("code").and_then(Value::as_i64).unwrap_or(-1),
            message: err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        });
    }
    Ok(Some(value.get("result").cloned().unwrap_or(Value::Null)))
}

/// Callers waiting on a response, keyed by JSON-RPC id.
type Pending = Arc<StdMutex<HashMap<i64, oneshot::Sender<Result<Value, McpError>>>>>;

/// Removes a pending entry when the waiting future is dropped (e.g. on
/// timeout), so a request that never gets a response cannot leak.
struct PendingGuard {
    pending: Pending,
    id: i64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}

/// An MCP client over stdio.
///
/// A background task reads the server's stdout and routes each JSON-RPC response
/// to the caller waiting on that id, so many requests can be in flight at once
/// and one slow call never blocks the others.
pub struct StdioMcpClient {
    child: StdMutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: Pending,
    next_id: AtomicI64,
    reader: tokio::task::JoinHandle<()>,
}

impl StdioMcpClient {
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<Self, McpError> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Protocol("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Protocol("no stdout".into()))?;

        let pending: Pending = Arc::new(StdMutex::new(HashMap::new()));
        let reader_pending = pending.clone();
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                    continue;
                };
                let Some(id) = value.get("id").and_then(Value::as_i64) else {
                    continue;
                };
                let result = parse_message(value, id).map(|result| result.unwrap_or(Value::Null));
                if let Some(sender) = reader_pending.lock().unwrap().remove(&id) {
                    let _ = sender.send(result);
                }
            }
            // The server closed stdout: wake every waiter with a closed error.
            reader_pending.lock().unwrap().clear();
        });

        Ok(Self {
            child: StdMutex::new(child),
            stdin: Mutex::new(stdin),
            pending,
            next_id: AtomicI64::new(1),
            reader,
        })
    }

    /// Whether the server process is still running. A child that has exited (or
    /// was killed) reports `false`, so the caller can reconnect.
    pub fn is_running(&self) -> bool {
        matches!(self.child.lock().unwrap().try_wait(), Ok(None))
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        let _guard = PendingGuard {
            pending: self.pending.clone(),
            id,
        };

        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut line = serde_json::to_string(&message).unwrap();
        line.push('\n');
        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
        }

        receiver
            .await
            .map_err(|_| McpError::Protocol("server closed the connection".into()))?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let mut line = serde_json::to_string(&message).unwrap();
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn initialize(&self) -> Result<Value, McpError> {
        let result = self.request("initialize", initialize_params()).await?;
        self.notify("notifications/initialized", json!({})).await?;
        Ok(result)
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        tools_from(result)
    }

    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value, McpError> {
        self.request("tools/call", json!({ "name": name, "arguments": args }))
            .await
    }
}

impl Drop for StdioMcpClient {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// A Streamable HTTP MCP client: each JSON-RPC message is POSTed to the endpoint
/// and the response is read as either `application/json` or an SSE stream.
pub struct HttpMcpClient {
    http: reqwest::Client,
    url: String,
    session_id: StdMutex<Option<String>>,
    next_id: AtomicI64,
}

impl HttpMcpClient {
    pub fn connect(url: &str) -> Result<Self, McpError> {
        if url.trim().is_empty() {
            return Err(McpError::Protocol("http server has no url".into()));
        }
        Ok(Self {
            http: reqwest::Client::builder().build()?,
            url: url.to_string(),
            session_id: StdMutex::new(None),
            next_id: AtomicI64::new(1),
        })
    }

    pub fn is_running(&self) -> bool {
        true
    }

    /// POST a JSON-RPC message, capturing any `Mcp-Session-Id` the server hands
    /// back so later requests can echo it.
    async fn post(&self, body: &Value) -> Result<reqwest::Response, McpError> {
        let session = self.session_id.lock().unwrap().clone();
        let mut request = self
            .http
            .post(&self.url)
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(body);
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }

        let response = request.send().await?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            *self.session_id.lock().unwrap() = Some(session.to_string());
        }
        Ok(response)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let response = self.post(&body).await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(McpError::Protocol(format!("http {status}: {text}")));
        }

        let is_sse = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.starts_with("text/event-stream"))
            .unwrap_or(false);

        if is_sse {
            let mut stream = response.bytes_stream().eventsource();
            while let Some(event) = stream.next().await {
                let event = event.map_err(|e| McpError::Protocol(format!("sse error: {e}")))?;
                if event.data.is_empty() {
                    continue;
                }
                let value: Value = serde_json::from_str(&event.data)
                    .map_err(|e| McpError::Protocol(format!("invalid json: {e}")))?;
                if let Some(result) = parse_message(value, id)? {
                    return Ok(result);
                }
            }
            return Err(McpError::Protocol(
                "sse stream ended without a response".into(),
            ));
        }

        let value: Value = response.json().await?;
        parse_message(value, id)?
            .ok_or_else(|| McpError::Protocol("response id did not match the request".into()))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let body = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let response = self.post(&body).await?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(McpError::Protocol(format!("http {status}: {text}")));
        }
        Ok(())
    }

    pub async fn initialize(&self) -> Result<Value, McpError> {
        let result = self.request("initialize", initialize_params()).await?;
        self.notify("notifications/initialized", json!({})).await?;
        Ok(result)
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        tools_from(result)
    }

    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value, McpError> {
        self.request("tools/call", json!({ "name": name, "arguments": args }))
            .await
    }
}
