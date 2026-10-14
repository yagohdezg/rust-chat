//! Pluggable sandbox for executing untrusted, model-generated code.
//!
//! BoxLite is the only execution backend: [`BoxliteBackend`] talks to a BoxLite
//! server (`boxlite serve`) and runs each request in a hardware-isolated
//! microVM. [`HttpSandboxBackend`] is a client for a remote `sandboxd` service,
//! which in turn owns a [`BoxliteBackend`]. Callers depend only on
//! [`SandboxBackend`].

pub mod boxlite;
pub mod egress;
pub mod http;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use boxlite::BoxliteBackend;
pub use egress::{parse_egress_allow, validate_egress_allow};
pub use http::HttpSandboxBackend;

/// Name prefixes marking a BoxLite box as owned by this deployment.
///
/// Boxes are named on creation so the warm-pool reconciler can tell ours from
/// foreign boxes on a shared node: a `pool`/`computer` box whose handle is no
/// longer tracked is a leak to reclaim, while `ephemeral` boxes belong to
/// short-lived runs and are left alone.
pub mod box_names {
    use uuid::Uuid;

    pub const POOL_PREFIX: &str = "rustchat-pool-";
    pub const COMPUTER_PREFIX: &str = "rustchat-computer-";
    pub const EPHEMERAL_PREFIX: &str = "rustchat-ephemeral-";

    pub fn pool() -> String {
        format!("{POOL_PREFIX}{}", Uuid::new_v4().simple())
    }

    pub fn computer() -> String {
        format!("{COMPUTER_PREFIX}{}", Uuid::new_v4().simple())
    }

    pub fn ephemeral() -> String {
        format!("{EPHEMERAL_PREFIX}{}", Uuid::new_v4().simple())
    }
}

/// A box as listed by a node: its opaque handle, optional deployment name, and
/// creation time (used to avoid racing a just-created box during reconcile).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxSummary {
    pub handle: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Strength of the isolation boundary around a sandboxed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IsolationLevel {
    /// Shares the host kernel (containers, namespaces, seccomp).
    SharedKernel,
    /// A user-space kernel intercepts syscalls (e.g. gVisor).
    UserSpaceKernel,
    /// A dedicated kernel in a virtual machine (Firecracker/BoxLite).
    MicroVm,
    /// Execution happens behind a remote service (e.g. `sandboxd`); the real
    /// isolation level is decided by the remote backend.
    Remote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSpec {
    pub image: String,
    pub timeout_seconds: u64,
    pub memory_mb: u64,
    pub cpus: f64,
    /// Cap on the box's writable disk in GiB. `None` leaves it unbounded (the
    /// disk is sparse and grows on demand).
    #[serde(default)]
    pub disk_gb: Option<u32>,
    /// Whether the sandbox may reach the network. Defaults to `false`.
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub egress_allow: Vec<String>,
}

impl Default for SandboxSpec {
    fn default() -> Self {
        Self {
            image: "python:3.12-slim".into(),
            timeout_seconds: 30,
            memory_mb: 512,
            cpus: 1.0,
            disk_gb: None,
            network: false,
            egress_allow: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxFile {
    pub name: String,
    /// Base64-encoded file contents.
    pub content_b64: String,
}

/// An output file that was collected from a sandbox but not handed back, with
/// the reason it was dropped (oversized, unsafe path, disallowed type, ...).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectedFile {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecRequest {
    /// Language identifier, e.g. `python`, `javascript`, `bash`, `go`, `rust`.
    pub language: String,
    pub code: String,
    /// Input files materialized into the workspace before the run.
    #[serde(default)]
    pub files: Vec<SandboxFile>,
    /// Extra workspace paths to collect as output files after the run, on top of
    /// the conventional output directory (see [`OUTPUT_DIR`]).
    #[serde(default)]
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    /// Set when captured stdout or stderr hit the size cap and was clipped.
    #[serde(default)]
    pub truncated: bool,
    /// Files collected from the workspace after the run (e.g. from
    /// [`OUTPUT_DIR`]). Base64-encoded, ready to persist or hand back.
    #[serde(default)]
    pub files: Vec<SandboxFile>,
    /// Output files that were found but not handed back, with the reason. Lets a
    /// caller surface a rejected output instead of silently losing it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected_files: Vec<RejectedFile>,
}

/// A raw command to run interactively inside a sandbox (argv + environment).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommandSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Optional image override; `None` uses the backend's configured image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// Which output stream an [`OutputChunk`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputChannel {
    Stdout,
    Stderr,
}

/// One chunk of a session's output.
#[derive(Debug, Clone)]
pub struct OutputChunk {
    pub channel: OutputChannel,
    pub data: Vec<u8>,
}

/// A live, bidirectional command running inside a sandbox: stdin in,
/// stdout/stderr out. Dropping the session tears down its sandbox.
#[async_trait::async_trait]
pub trait SandboxSession: Send + Sync {
    /// Append raw bytes to the command's stdin.
    async fn write_stdin(&self, data: &[u8]) -> Result<(), SandboxError>;

    /// Read the next output chunk. `None` once the command has exited or the
    /// stream closed.
    async fn read_output(&self) -> Option<OutputChunk>;

    /// Terminate the session and release its sandbox. Idempotent.
    async fn close(&self);
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
    ///
    /// The backend holds its own [`SandboxSpec`] (image, limits, timeout), so
    /// callers only supply the request.
    async fn run(&self, request: &ExecRequest) -> Result<ExecResult, SandboxError>;

    /// Open a long-lived interactive command in a fresh sandbox, bridging
    /// stdin/stdout/stderr until the returned session is closed.
    async fn open_session(
        &self,
        command: &CommandSpec,
    ) -> Result<Box<dyn SandboxSession>, SandboxError>;
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

/// In-box directory the upload lands in and exec runs from.
const WORK_DIR: &str = "/app";

/// In-box directory whose contents are collected back as output files after a
/// run. Callers can also request additional paths per request via
/// [`ExecRequest::outputs`].
pub const OUTPUT_DIR: &str = "/app/output";

/// BoxLite's `boxlite serve` unpacks a `PUT /files` archive into a temp
/// `extracted/` directory and then copies that directory *into* `path`, so
/// entries land at `<path>/extracted/...` instead of `<path>/...`. Flatten it
/// before running. This is a no-op once upstream copies the directory contents.
fn flatten_upload() -> String {
    format!(
        "cp -a {WORK_DIR}/extracted/. {WORK_DIR}/ 2>/dev/null; \
         rm -rf {WORK_DIR}/extracted 2>/dev/null; cd {WORK_DIR} 2>/dev/null"
    )
}

/// Split a request into a BoxLite `command` + `args` pair and the source file
/// extension, using `/app` as the in-box working directory.
pub(crate) fn program(language: &str) -> Option<(String, Vec<String>, &'static str)> {
    let lower = language.to_ascii_lowercase();
    let (mut argv, ext) = language_command(&lower)?;
    let inner = match lower.as_str() {
        "rust" => format!("rustc main.{ext} -o /tmp/a && /tmp/a"),
        "c" => format!("cc main.{ext} -o /tmp/a && /tmp/a"),
        "cpp" | "c++" => format!("c++ main.{ext} -o /tmp/a && /tmp/a"),
        "typescript" | "ts" => format!("npx --yes tsx main.{ext}"),
        _ => {
            let mut parts: Vec<String> = argv.drain(..).map(String::from).collect();
            parts.push(format!("main.{ext}"));
            parts.join(" ")
        }
    };
    let script = format!("{}; {inner}", flatten_upload());
    Some(("sh".into(), vec!["-c".into(), script], ext))
}
