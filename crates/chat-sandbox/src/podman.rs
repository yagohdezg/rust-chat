use std::process::Stdio;

use base64::Engine as _;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::{
    language_command, scratch_dir, ExecRequest, ExecResult, IsolationLevel, SandboxBackend,
    SandboxError, SandboxSpec,
};

/// Rootless Podman backend.
///
/// Each run creates an ephemeral container (`--rm`), network disabled by default,
/// with cgroup memory/CPU limits and a bind-mounted scratch directory at
/// `/mnt/data`. Requires rootless Podman with `crun` on the host.
pub struct PodmanBackend {
    binary: String,
}

impl Default for PodmanBackend {
    fn default() -> Self {
        Self {
            binary: "podman".into(),
        }
    }
}

impl PodmanBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Args appended after the image reference to run the request.
    fn program_args(language: &str, ext: &str) -> Vec<String> {
        match language.to_ascii_lowercase().as_str() {
            "rust" => vec![
                "sh".into(),
                "-c".into(),
                format!("rustc main.{ext} -o /tmp/a && /tmp/a"),
            ],
            "c" => vec![
                "sh".into(),
                "-c".into(),
                format!("cc main.{ext} -o /tmp/a && /tmp/a"),
            ],
            "cpp" | "c++" => {
                vec![
                    "sh".into(),
                    "-c".into(),
                    format!("c++ main.{ext} -o /tmp/a && /tmp/a"),
                ]
            }
            "typescript" | "ts" => {
                vec![
                    "sh".into(),
                    "-c".into(),
                    format!("npx --yes tsx main.{ext}"),
                ]
            }
            _ => {
                let (argv, _) = language_command(language).unwrap_or((vec!["sh"], "sh"));
                let mut args: Vec<String> = argv.into_iter().map(String::from).collect();
                args.push(format!("/mnt/data/main.{ext}"));
                args
            }
        }
    }
}

#[async_trait::async_trait]
impl SandboxBackend for PodmanBackend {
    fn name(&self) -> &str {
        "podman"
    }

    fn isolation(&self) -> IsolationLevel {
        IsolationLevel::SharedKernel
    }

    async fn run(
        &self,
        spec: &SandboxSpec,
        request: &ExecRequest,
    ) -> Result<ExecResult, SandboxError> {
        let (_, ext) = language_command(&request.language)
            .ok_or_else(|| SandboxError::UnsupportedLanguage(request.language.clone()))?;

        let dir = scratch_dir();
        tokio::fs::create_dir_all(&dir).await?;

        // Materialize source + input files.
        tokio::fs::write(dir.join(format!("main.{ext}")), request.code.as_bytes()).await?;
        for file in &request.files {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&file.content_b64)
                .map_err(|e| {
                    SandboxError::Other(format!("invalid base64 in {}: {e}", file.name))
                })?;
            let path = dir.join(&file.name);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(path, bytes).await?;
        }

        let mut cmd = Command::new(&self.binary);
        cmd.arg("run")
            .arg("--rm")
            .arg("-i")
            .arg("--network")
            .arg(if spec.network { "bridge" } else { "none" })
            .arg("--memory")
            .arg(format!("{}m", spec.memory_mb))
            .arg("--cpus")
            .arg(spec.cpus.to_string())
            .arg("--pids-limit")
            .arg("256")
            .arg("--security-opt")
            .arg("no-new-privileges")
            .arg("--cap-drop")
            .arg("ALL")
            .arg("-v")
            .arg(format!("{}:/mnt/data:Z", dir.display()))
            .arg("-w")
            .arg("/mnt/data")
            .arg(&spec.image);

        for arg in Self::program_args(&request.language, ext) {
            cmd.arg(arg);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let child = cmd.spawn().map_err(|e| {
            SandboxError::Unavailable(format!("failed to spawn `{}`: {e}", self.binary))
        })?;

        let result = timeout(
            Duration::from_secs(spec.timeout_seconds),
            child.wait_with_output(),
        )
        .await;

        // Always clean up the scratch directory.
        let _ = tokio::fs::remove_dir_all(&dir).await;

        match result {
            Ok(Ok(output)) => Ok(ExecResult {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                timed_out: false,
            }),
            Ok(Err(e)) => Err(SandboxError::Io(e)),
            Err(_) => Ok(ExecResult {
                exit_code: -1,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: true,
            }),
        }
    }
}
