//! Optional malware-scanning hook for uploads.
//!
//! An operator can point `UPLOAD_SCAN_COMMAND` at an external scanner (e.g.
//! `clamdscan --no-summary --stdout -`). The command receives the upload on
//! stdin and accepts it with exit 0; any non-zero exit rejects the upload. With
//! no command configured, every upload is accepted.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use chat_core::ChatError;

use crate::error::ApiError;

const SCAN_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs an operator-configured external scanner on upload bytes.
pub struct UploadScanner {
    command: Option<String>,
}

impl UploadScanner {
    pub fn new(command: Option<String>) -> Self {
        Self {
            command: command.filter(|command| !command.trim().is_empty()),
        }
    }

    pub fn enabled(&self) -> bool {
        self.command.is_some()
    }

    /// Scan `bytes`, rejecting the upload when the scanner does not exit 0.
    /// A scanner that cannot start or times out fails closed.
    pub async fn scan(&self, filename: &str, bytes: &[u8]) -> Result<(), ApiError> {
        let Some(command) = self.command.as_deref() else {
            return Ok(());
        };
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .env("UPLOAD_FILENAME", filename)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| {
                ChatError::Internal(anyhow::anyhow!("upload scanner failed to start: {err}"))
            })?;

        let run = async {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(bytes).await?;
                drop(stdin);
            }
            child.wait().await
        };

        match tokio::time::timeout(SCAN_TIMEOUT, run).await {
            Ok(Ok(status)) if status.success() => Ok(()),
            Ok(Ok(status)) => {
                tracing::warn!(%status, filename, "upload rejected by malware scan");
                Err(ChatError::BadRequest("upload rejected by malware scan".into()).into())
            }
            Ok(Err(err)) => {
                Err(ChatError::Internal(anyhow::anyhow!("upload scanner failed: {err}")).into())
            }
            Err(_) => Err(ChatError::Internal(anyhow::anyhow!("upload scanner timed out")).into()),
        }
    }
}
