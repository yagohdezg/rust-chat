//! gVisor backend — a user-space kernel intercepts syscalls, so a run is
//! isolated without a virtual machine or `/dev/kvm`.
//!
//! Each request is turned into an OCI bundle in a temporary directory and run
//! with gVisor's `runsc`. `runsc` does not pull images: the deployment unpacks
//! each configured image into a rootfs directory under its configured image root
//! (`<image_root>/<image>`). The workspace is bind-mounted at `/app`, so the
//! language/program mapping and the `output/` convention match the BoxLite
//! backend. Everything is best-effort: a missing `runsc` or rootfs reports
//! [`SandboxError::Unavailable`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;

use crate::boxlite::{normalize_entry_name, push_capped, MAX_OUTPUT_FILES, MAX_OUTPUT_FILE_BYTES};
use crate::{
    program, CommandSpec, ExecRequest, ExecResult, IsolationLevel, OutputChannel, OutputChunk,
    RejectedFile, SandboxBackend, SandboxError, SandboxFile, SandboxSession, SandboxSpec,
};

/// `runsc` binary, resolved from `PATH`.
const RUNSC: &str = "runsc";

/// In-container directory the workspace is bind-mounted at.
const WORK_DIR: &str = "/app";

/// Default `PATH` handed to the container process.
const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// gVisor backend — one user-space-kernel sandbox per run.
///
/// Runs each request as an OCI container via `runsc` with networking disabled
/// and a wall-clock timeout. Unlike [`crate::BoxliteBackend`] there is no
/// server to talk to: `runsc` is spawned directly.
pub struct GvisorBackend {
    spec: SandboxSpec,
    image_root: PathBuf,
}

impl GvisorBackend {
    /// Create a backend that unpacks images under `image_root`.
    pub fn new(spec: SandboxSpec, image_root: impl Into<PathBuf>) -> Self {
        Self {
            spec,
            image_root: image_root.into(),
        }
    }

    pub fn spec(&self) -> &SandboxSpec {
        &self.spec
    }

    fn rootfs(&self, image: &str) -> Result<PathBuf, SandboxError> {
        let path = self.image_root.join(image);
        if path.is_dir() {
            Ok(path)
        } else {
            Err(SandboxError::Unavailable(format!(
                "gVisor rootfs for image {image} not found at {}",
                path.display()
            )))
        }
    }
}

#[async_trait::async_trait]
impl SandboxBackend for GvisorBackend {
    fn name(&self) -> &str {
        "gvisor"
    }

    fn isolation(&self) -> IsolationLevel {
        IsolationLevel::UserSpaceKernel
    }

    async fn run(&self, request: &ExecRequest) -> Result<ExecResult, SandboxError> {
        let (command, args, ext) = program(&request.language)
            .ok_or_else(|| SandboxError::UnsupportedLanguage(request.language.clone()))?;
        let rootfs = self.rootfs(&self.spec.image)?;

        let temp = TempDir::create()?;
        let workspace = temp.path().join("app");
        write_workspace(&workspace, request, ext)?;

        let mut argv = vec![command];
        argv.extend(args);
        let config = bundle_config(&rootfs, &workspace, argv, &BTreeMap::new(), &self.spec);
        let bundle = write_bundle(temp.path(), &config)?;
        let state = temp.path().join("runsc");
        std::fs::create_dir_all(&state)?;
        let id = container_id();

        let mut child = spawn(&state, &bundle, &id, Stdio::null())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::Other("missing stdout pipe".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::Other("missing stderr pipe".into()))?;
        let out_task = tokio::spawn(read_capped(stdout));
        let err_task = tokio::spawn(read_capped(stderr));

        let deadline = Duration::from_secs(self.spec.timeout_seconds.max(1));
        let mut timed_out = false;
        let exit_code = match tokio::time::timeout(deadline, child.wait()).await {
            Ok(Ok(status)) => status.code().unwrap_or(-1),
            Ok(Err(error)) => return Err(SandboxError::Io(error)),
            Err(_) => {
                timed_out = true;
                let _ = child.kill().await;
                let _ = child.wait().await;
                -1
            }
        };

        let (stdout, stdout_truncated) = join_output(out_task).await?;
        let (stderr, stderr_truncated) = join_output(err_task).await?;
        let (files, rejected_files) = collect_outputs(&workspace, request);

        Ok(ExecResult {
            exit_code,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            timed_out,
            truncated: stdout_truncated || stderr_truncated,
            files,
            rejected_files,
        })
    }

    async fn open_session(
        &self,
        command: &CommandSpec,
    ) -> Result<Box<dyn SandboxSession>, SandboxError> {
        let image = command
            .image
            .as_deref()
            .map(str::trim)
            .filter(|image| !image.is_empty())
            .unwrap_or(self.spec.image.as_str());
        let rootfs = self.rootfs(image)?;

        let temp = TempDir::create()?;
        let workspace = temp.path().join("app");
        std::fs::create_dir_all(&workspace)?;

        let mut argv = vec![command.command.clone()];
        argv.extend(command.args.iter().cloned());
        let config = bundle_config(&rootfs, &workspace, argv, &command.env, &self.spec);
        let bundle = write_bundle(temp.path(), &config)?;
        let state = temp.path().join("runsc");
        std::fs::create_dir_all(&state)?;
        let id = container_id();

        let mut child = spawn(&state, &bundle, &id, Stdio::piped())?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        if let Some(stdout) = stdout {
            tokio::spawn(stream_output(stdout, OutputChannel::Stdout, tx.clone()));
        }
        if let Some(stderr) = stderr {
            tokio::spawn(stream_output(stderr, OutputChannel::Stderr, tx.clone()));
        }
        drop(tx);

        Ok(Box::new(GvisorSession {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            rx: Mutex::new(rx),
            _temp: temp,
            closed: AtomicBool::new(false),
        }))
    }
}

/// A live interactive command backed by a foreground `runsc run`.
///
/// The child's stdout/stderr are pumped into a channel by two reader tasks;
/// stdin is written straight to the child. Closing kills `runsc`, which tears
/// down the container.
struct GvisorSession {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    rx: Mutex<tokio::sync::mpsc::Receiver<OutputChunk>>,
    _temp: TempDir,
    closed: AtomicBool,
}

#[async_trait::async_trait]
impl SandboxSession for GvisorSession {
    async fn write_stdin(&self, data: &[u8]) -> Result<(), SandboxError> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| SandboxError::Other("session is closed".into()))?;
        stdin.write_all(data).await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn read_output(&self) -> Option<OutputChunk> {
        self.rx.lock().await.recv().await
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        *self.stdin.lock().await = None;
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

impl Drop for GvisorSession {
    fn drop(&mut self) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.start_kill();
        }
    }
}

/// A temporary directory removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn create() -> Result<Self, SandboxError> {
        let path = std::env::temp_dir().join(format!("rustchat-gvisor-{}", container_id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn container_id() -> String {
    format!("rustchat-{}", uuid::Uuid::new_v4().simple())
}

fn spawn(state: &Path, bundle: &Path, id: &str, stdin: Stdio) -> Result<Child, SandboxError> {
    Command::new(RUNSC)
        .arg("--root")
        .arg(state)
        .arg("--network=none")
        .arg("run")
        .arg("--bundle")
        .arg(bundle)
        .arg(id)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(spawn_error)
}

fn spawn_error(error: std::io::Error) -> SandboxError {
    if error.kind() == std::io::ErrorKind::NotFound {
        SandboxError::Unavailable("runsc is not installed".into())
    } else {
        SandboxError::Io(error)
    }
}

fn write_workspace(dir: &Path, request: &ExecRequest, ext: &str) -> Result<(), SandboxError> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("main.{ext}")), request.code.as_bytes())?;
    for file in &request.files {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&file.content_b64)
            .map_err(|e| SandboxError::Other(format!("invalid base64 in {}: {e}", file.name)))?;
        let Some(relative) = workspace_relative(&file.name) else {
            return Err(SandboxError::Other(format!(
                "unsafe input path: {}",
                file.name
            )));
        };
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &bytes)?;
    }
    Ok(())
}

fn write_bundle(dir: &Path, config: &serde_json::Value) -> Result<PathBuf, SandboxError> {
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle)?;
    let bytes = serde_json::to_vec(config)
        .map_err(|e| SandboxError::Other(format!("invalid OCI config: {e}")))?;
    std::fs::write(bundle.join("config.json"), bytes)?;
    Ok(bundle)
}

fn bundle_config(
    rootfs: &Path,
    workspace: &Path,
    args: Vec<String>,
    env: &BTreeMap<String, String>,
    spec: &SandboxSpec,
) -> serde_json::Value {
    let mut environment = vec![format!("PATH={DEFAULT_PATH}")];
    environment.extend(env.iter().map(|(key, value)| format!("{key}={value}")));

    let mut resources = serde_json::json!({
        "memory": { "limit": spec.memory_mb.max(128) * 1024 * 1024 },
    });
    if spec.cpus > 0.0 {
        resources["cpu"] = serde_json::json!({
            "quota": (spec.cpus * 100_000.0).round() as i64,
            "period": 100_000,
        });
    }

    serde_json::json!({
        "ociVersion": "1.0.2",
        "process": {
            "terminal": false,
            "cwd": WORK_DIR,
            "args": args,
            "env": environment,
            "user": { "uid": 0, "gid": 0 },
        },
        "root": { "path": rootfs, "readonly": false },
        "hostname": "rustchat",
        "mounts": [
            { "destination": "/proc", "type": "proc", "source": "proc" },
            { "destination": "/dev", "type": "tmpfs", "source": "tmpfs",
              "options": ["nosuid", "strictatime", "mode=755", "size=65536k"] },
            { "destination": "/sys", "type": "sysfs", "source": "sysfs",
              "options": ["nosuid", "noexec", "nodev", "ro"] },
            { "destination": "/tmp", "type": "tmpfs", "source": "tmpfs",
              "options": ["nosuid", "nodev", "mode=1777", "size=64m"] },
            { "destination": WORK_DIR, "type": "bind", "source": workspace, "options": ["bind"] },
        ],
        "linux": {
            "namespaces": [
                { "type": "pid" },
                { "type": "ipc" },
                { "type": "uts" },
                { "type": "mount" },
                { "type": "network" },
            ],
            "resources": resources,
        },
    })
}

async fn read_capped<R: AsyncRead + Unpin>(mut reader: R) -> (Vec<u8>, bool) {
    let mut buffer = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => push_capped(&mut buffer, &chunk[..n], &mut truncated),
        }
    }
    (buffer, truncated)
}

async fn join_output(
    task: tokio::task::JoinHandle<(Vec<u8>, bool)>,
) -> Result<(Vec<u8>, bool), SandboxError> {
    task.await
        .map_err(|e| SandboxError::Other(format!("output reader failed: {e}")))
}

async fn stream_output<R: AsyncRead + Unpin>(
    mut reader: R,
    channel: OutputChannel,
    tx: tokio::sync::mpsc::Sender<OutputChunk>,
) {
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = OutputChunk {
                    channel,
                    data: buffer[..n].to_vec(),
                };
                if tx.send(chunk).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn collect_outputs(
    workspace: &Path,
    request: &ExecRequest,
) -> (Vec<SandboxFile>, Vec<RejectedFile>) {
    let mut roots = vec![workspace.join("output")];
    roots.extend(
        request
            .outputs
            .iter()
            .filter_map(|path| workspace_relative(path).map(|rel| workspace.join(rel))),
    );

    let mut files = Vec::new();
    let mut rejected = Vec::new();
    for root in roots {
        if files.len() >= MAX_OUTPUT_FILES {
            break;
        }
        collect_path(&root, workspace, &mut files, &mut rejected);
    }
    (files, rejected)
}

fn collect_path(
    path: &Path,
    workspace: &Path,
    files: &mut Vec<SandboxFile>,
    rejected: &mut Vec<RejectedFile>,
) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.is_file() {
        collect_file(path, workspace, files, rejected);
        return;
    }
    if !metadata.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        if files.len() >= MAX_OUTPUT_FILES {
            return;
        }
        collect_path(&entry.path(), workspace, files, rejected);
    }
}

fn collect_file(
    path: &Path,
    workspace: &Path,
    files: &mut Vec<SandboxFile>,
    rejected: &mut Vec<RejectedFile>,
) {
    let Ok(relative) = path.strip_prefix(workspace) else {
        return;
    };
    let Some(name) = normalize_entry_name(&relative.to_string_lossy()) else {
        rejected.push(RejectedFile {
            name: relative.to_string_lossy().into_owned(),
            reason: "unsafe output path".into(),
        });
        return;
    };
    if files.iter().any(|existing| existing.name == name) {
        return;
    }
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.len() > MAX_OUTPUT_FILE_BYTES {
        rejected.push(RejectedFile {
            name,
            reason: format!("exceeds the {MAX_OUTPUT_FILE_BYTES}-byte sandbox limit"),
        });
        return;
    }
    let Ok(data) = std::fs::read(path) else {
        return;
    };
    files.push(SandboxFile {
        name,
        content_b64: base64::engine::general_purpose::STANDARD.encode(&data),
    });
}

fn workspace_relative(path: &str) -> Option<PathBuf> {
    let trimmed = path.trim_start_matches('/');
    let relative = trimmed.strip_prefix("app/").unwrap_or(trimmed);
    if relative.is_empty() || relative.split('/').any(|part| part == "..") {
        return None;
    }
    Some(PathBuf::from(relative))
}
