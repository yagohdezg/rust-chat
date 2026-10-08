//! Pluggable sandbox for executing untrusted, model-generated code.
//!
//! Two backends are provided:
//!
//! * [`PodmanBackend`] — rootless Podman containers (`crun`). Fast and low-memory,
//!   but shares the host kernel with the workload. Suitable for local development
//!   and as a fallback.
//! * [`BoxliteBackend`] — one hardware-isolated microVM per run, via the BoxLite
//!   REST server (`boxlite serve`). This is the production backend and offers the
//!   strongest isolation (own kernel + seccomp + cgroups + egress allow-list).
//!
//! Callers depend only on [`SandboxBackend`], so switching backends is a config
//! change (`SANDBOX_BACKEND=podman|boxlite`).

pub mod boxlite;
pub mod podman;

use serde::{Deserialize, Serialize};

pub use boxlite::BoxliteBackend;
pub use podman::PodmanBackend;

/// Strength of the isolation boundary around a sandboxed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IsolationLevel {
    /// Shares the host kernel (containers, namespaces, seccomp).
    SharedKernel,
    /// A user-space kernel intercepts syscalls (e.g. gVisor).
    UserSpaceKernel,
    /// A dedicated kernel in a virtual machine (Firecracker/BoxLite).
    MicroVm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSpec {
    pub image: String,
    pub timeout_seconds: u64,
    pub memory_mb: u64,
    pub cpus: f64,
    /// Whether the sandbox may reach the network. Defaults to `false`.
    #[serde(default)]
    pub network: bool,
}

impl Default for SandboxSpec {
    fn default() -> Self {
        Self {
            image: "python:3.12-slim".into(),
            timeout_seconds: 30,
            memory_mb: 512,
            cpus: 1.0,
            network: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxFile {
    pub name: String,
    /// Base64-encoded file contents.
    pub content_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRequest {
    /// Language identifier, e.g. `python`, `javascript`, `bash`, `go`, `rust`.
    pub language: String,
    pub code: String,
    #[serde(default)]
    pub files: Vec<SandboxFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("unsupported language: {0}")]
    UnsupportedLanguage(String),
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    #[error("execution timed out after {0}s")]
    Timeout(u64),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("sandbox error: {0}")]
    Other(String),
}

#[async_trait::async_trait]
pub trait SandboxBackend: Send + Sync {
    fn name(&self) -> &str;

    fn isolation(&self) -> IsolationLevel;

    /// Run one request in a fresh sandbox and return its result.
    async fn run(
        &self,
        spec: &SandboxSpec,
        request: &ExecRequest,
    ) -> Result<ExecResult, SandboxError>;
}

/// Map a language identifier to (interpreter argv, source file extension).
pub(crate) fn language_command(language: &str) -> Option<(Vec<&'static str>, &'static str)> {
    Some(match language.to_ascii_lowercase().as_str() {
        "python" | "python3" | "py" => (vec!["python3"], "py"),
        "javascript" | "js" | "node" => (vec!["node"], "js"),
        "typescript" | "ts" => (vec!["npx", "tsx"], "ts"),
        "bash" | "sh" | "shell" => (vec!["bash"], "sh"),
        "ruby" => (vec!["ruby"], "rb"),
        "php" => (vec!["php"], "php"),
        "go" => (vec!["go", "run"], "go"),
        "rust" => (vec!["sh", "-c"], "rs"),
        "c" | "cpp" | "c++" => (vec!["sh", "-c"], "c"),
        _ => return None,
    })
}

/// Create a unique scratch directory for a sandbox run.
pub(crate) fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rust-chat-sbx-{}", uuid::Uuid::new_v4()));
    dir
}
