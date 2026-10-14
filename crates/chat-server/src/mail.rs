//! Outbound mail for the account flows (email verification, password reset).
//!
//! `MAIL_SEND_COMMAND` names a sendmail-compatible command that reads an RFC 822
//! message on stdin (`msmtp -t`, `sendmail -t`, ...). With no command
//! configured, nothing is delivered and a warning is logged, so a deployment
//! without an MTA still runs: tokens stay valid and rate-limited, only the
//! delivery step is skipped.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use chat_core::ChatError;

const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Delivers the account emails, through the operator's sendmail-compatible hook.
pub struct Mailer {
    command: Option<String>,
    from: String,
    public_url: String,
}

impl Mailer {
    pub fn new(command: Option<String>, from: String, public_url: String) -> Self {
        Self {
            command: command.filter(|command| !command.trim().is_empty()),
            from,
            public_url,
        }
    }

    /// Whether a delivery command is configured.
    pub fn enabled(&self) -> bool {
        self.command.is_some()
    }

    /// Absolute URL of a page in the web app, carrying a token in the query.
    pub fn link(&self, path: &str, token: &str) -> String {
        format!("{}{path}?token={token}", self.public_url)
    }

    /// Deliver one plain-text message. Delivery is best-effort: callers log and
    /// carry on when this fails, so a broken MTA never blocks an account flow.
    pub async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), ChatError> {
        let Some(command) = self.command.as_deref() else {
            tracing::warn!(
                to,
                subject,
                "MAIL_SEND_COMMAND is not set; message not delivered"
            );
            return Ok(());
        };
        let message = format!(
            "From: {}\r\nTo: {to}\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n",
            self.from
        );
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| {
                ChatError::Internal(anyhow::anyhow!("mail command failed to start: {err}"))
            })?;

        let run = async {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(message.as_bytes()).await?;
                drop(stdin);
            }
            child.wait().await
        };

        match tokio::time::timeout(SEND_TIMEOUT, run).await {
            Ok(Ok(status)) if status.success() => Ok(()),
            Ok(Ok(status)) => Err(ChatError::Internal(anyhow::anyhow!(
                "mail command exited with {status}"
            ))),
            Ok(Err(err)) => Err(ChatError::Internal(anyhow::anyhow!(
                "mail command failed: {err}"
            ))),
            Err(_) => Err(ChatError::Internal(anyhow::anyhow!(
                "mail command timed out"
            ))),
        }
    }
}
