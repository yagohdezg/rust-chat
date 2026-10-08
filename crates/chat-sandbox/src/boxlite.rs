use crate::{ExecRequest, ExecResult, IsolationLevel, SandboxBackend, SandboxError, SandboxSpec};

/// BoxLite backend — one hardware-isolated microVM per run.
///
/// This talks to a running BoxLite server (`boxlite serve`, default
/// `http://localhost:8100`) rather than embedding the `boxlite` crate, so the
/// API process stays lean and sandboxes can be scheduled on dedicated hosts.
///
/// Status: scaffolded. The REST contract is tracked in the BoxLite OpenAPI spec
/// (`openapi/box.openapi.yaml`); wire the concrete endpoints here once pinned.
pub struct BoxliteBackend {
    base_url: String,
    http: reqwest::Client,
}

impl BoxliteBackend {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
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

    async fn run(
        &self,
        _spec: &SandboxSpec,
        _request: &ExecRequest,
    ) -> Result<ExecResult, SandboxError> {
        // Verify the server is reachable, then fail closed until the exec
        // endpoints are wired. This is intentional: never silently fall back to
        // weaker isolation than the operator selected.
        match self
            .http
            .get(format!("{}/v1/boxes", self.base_url))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => Err(SandboxError::Other(
                "BoxLite backend reachable but exec integration is not yet wired".into(),
            )),
            Ok(resp) => Err(SandboxError::Unavailable(format!(
                "BoxLite server returned {}",
                resp.status()
            ))),
            Err(e) => Err(SandboxError::Unavailable(format!(
                "cannot reach BoxLite at {}: {e}",
                self.base_url
            ))),
        }
    }
}
