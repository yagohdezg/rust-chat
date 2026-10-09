//! Sandbox nodes and the router that places computers on them.
//!
//! A [`ComputerNode`] is one host that can run persistent boxes (today a
//! `sandboxd` instance backed by BoxLite). [`NodeRouter`] indexes nodes by name;
//! the placement registry stores a node *name*, and the router turns it back
//! into a callable node — so routing never depends on a sticky load balancer.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use chat_sandbox::{ExecRequest, ExecResult, SandboxError};

use crate::error::{ComputerError, Result};

/// One host able to run persistent per-user boxes.
#[async_trait]
pub trait ComputerNode: Send + Sync {
    /// Stable node name, matching what is stored in the placement registry.
    fn name(&self) -> &str;

    /// Provision a new blank box, returning its opaque handle.
    async fn create(&self) -> Result<String>;

    /// Run a request inside an existing box, leaving the box alive.
    async fn exec(&self, handle: &str, request: &ExecRequest) -> Result<ExecResult>;

    /// Remove a box and free its resources. Idempotent.
    async fn destroy(&self, handle: &str) -> Result<()>;

    /// Cheap reachability probe for health checks.
    async fn health(&self) -> Result<()>;
}

/// HTTP client for one `sandboxd` node.
///
/// Speaks the node's persistent-computer endpoints: `POST /v1/computers`,
/// `POST /v1/computers/{handle}/exec` and `DELETE /v1/computers/{handle}`.
pub struct HttpComputerNode {
    name: String,
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl HttpComputerNode {
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        token: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.filter(|t| !t.trim().is_empty()),
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1/{path}", self.base_url)
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// Send a request, mapping transport and non-2xx responses to errors.
    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        what: &str,
    ) -> Result<reqwest::Response> {
        let resp = request.send().await.map_err(|e| {
            ComputerError::Node(SandboxError::Transport(format!(
                "{what} request failed: {e}"
            )))
        })?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ComputerError::Node(SandboxError::Unavailable(
                "sandboxd rejected the bearer token".into(),
            )));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ComputerError::Node(SandboxError::Unavailable(format!(
                "{what} failed: {status} {body}"
            ))));
        }
        Ok(resp)
    }
}

#[derive(serde::Deserialize)]
struct HandleResponse {
    handle: String,
}

#[async_trait]
impl ComputerNode for HttpComputerNode {
    fn name(&self) -> &str {
        &self.name
    }

    async fn create(&self) -> Result<String> {
        let resp = self
            .send(
                self.auth(self.http.post(self.url("computers"))),
                "create computer",
            )
            .await?;
        let body: HandleResponse = resp.json().await.map_err(|e| {
            ComputerError::Node(SandboxError::Transport(format!(
                "invalid create response: {e}"
            )))
        })?;
        Ok(body.handle)
    }

    async fn exec(&self, handle: &str, request: &ExecRequest) -> Result<ExecResult> {
        let url = self.url(&format!("computers/{handle}/exec"));
        let resp = self
            .send(
                self.auth(self.http.post(url).json(request)),
                "exec computer",
            )
            .await?;
        resp.json::<ExecResult>().await.map_err(|e| {
            ComputerError::Node(SandboxError::Transport(format!(
                "invalid exec response: {e}"
            )))
        })
    }

    async fn destroy(&self, handle: &str) -> Result<()> {
        let url = self.url(&format!("computers/{handle}"));
        self.send(self.auth(self.http.delete(url)), "destroy computer")
            .await?;
        Ok(())
    }

    async fn health(&self) -> Result<()> {
        let url = format!("{}/health", self.base_url);
        self.send(self.http.get(url), "health").await?;
        Ok(())
    }
}

/// Name -> node lookup used by the orchestrator to resolve placements.
#[derive(Clone, Default)]
pub struct NodeRouter {
    nodes: BTreeMap<String, Arc<dyn ComputerNode>>,
}

impl NodeRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node, keyed by its own `name()`.
    pub fn with_node(mut self, node: Arc<dyn ComputerNode>) -> Self {
        self.nodes.insert(node.name().to_string(), node);
        self
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn ComputerNode>> {
        self.nodes.get(name)
    }

    /// Every configured node name, in stable (sorted) order.
    pub fn names(&self) -> Vec<String> {
        self.nodes.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}
