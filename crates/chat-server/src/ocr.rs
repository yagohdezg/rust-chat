//! Optional OCR hook for documents that carry no readable text layer.
//!
//! `doctext` handles formats whose text is stored as text. Scans, screenshots
//! and image-only PDFs are pixels, so an operator can point `OCR_COMMAND` at an
//! external recognizer (a script wrapping `tesseract`, say). The command
//! receives the document as a file path in `$1` and its name in `OCR_FILENAME`,
//! and prints the recognized text on stdout. With no command configured nothing
//! is recognized, and any failure is logged and skipped rather than raised.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use uuid::Uuid;

const OCR_TIMEOUT: Duration = Duration::from_secs(20);

/// Extensions whose contents only become text through recognition.
const RECOGNIZABLE: &[&str] = &[
    "pdf", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff",
];

/// Runs an operator-configured external OCR command on image-like documents.
pub struct Ocr {
    command: Option<String>,
}

impl Ocr {
    pub fn new(command: Option<String>) -> Self {
        Self {
            command: command.filter(|command| !command.trim().is_empty()),
        }
    }

    pub fn enabled(&self) -> bool {
        self.command.is_some()
    }

    /// Whether a file of this name can need recognition at all.
    pub fn recognizes(filename: &str) -> bool {
        filename
            .rsplit('.')
            .next()
            .map(|ext| RECOGNIZABLE.contains(&ext.to_ascii_lowercase().as_str()))
            .unwrap_or(false)
    }

    /// Recognize the text in `bytes`, or `None` when no command is configured,
    /// the command fails, or it recognizes nothing.
    ///
    /// The document is staged as a temporary file so the command can rasterize
    /// a PDF page before reading it, and the file is removed afterwards.
    pub async fn recognize(&self, filename: &str, bytes: &[u8]) -> Option<String> {
        let command = self.command.as_deref()?;
        let path = temp_path(filename);
        if let Err(err) = tokio::fs::write(&path, bytes).await {
            tracing::warn!(error = %err, file = %filename, "could not stage document for OCR");
            return None;
        }
        let text = self.run(command, filename, &path).await;
        if let Err(err) = tokio::fs::remove_file(&path).await {
            tracing::warn!(error = %err, file = %filename, "could not remove OCR input");
        }
        text
    }

    async fn run(&self, command: &str, filename: &str, path: &PathBuf) -> Option<String> {
        let child = match Command::new("sh")
            .arg("-c")
            .arg(format!("{command} \"$1\""))
            .arg("sh")
            .arg(path)
            .env("OCR_FILENAME", filename)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!(error = %err, file = %filename, "OCR command failed to start");
                return None;
            }
        };

        match tokio::time::timeout(OCR_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                (!text.is_empty()).then_some(text)
            }
            Ok(Ok(output)) => {
                tracing::warn!(status = %output.status, file = %filename, "OCR command failed");
                None
            }
            Ok(Err(err)) => {
                tracing::warn!(error = %err, file = %filename, "OCR command failed");
                None
            }
            Err(_) => {
                tracing::warn!(file = %filename, "OCR timed out");
                None
            }
        }
    }
}

/// A unique path in the temporary directory carrying the document's extension,
/// which is how a command script tells a PDF from a PNG.
fn temp_path(filename: &str) -> PathBuf {
    let ext: String = filename
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    let ext = if ext.is_empty() { "bin".into() } else { ext };
    std::env::temp_dir().join(format!("ocr-{}.{ext}", Uuid::new_v4()))
}
