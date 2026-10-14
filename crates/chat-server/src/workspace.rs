//! Conversation file workspaces: the bridge between a user's personal file
//! storage and the sandbox.
//!
//! `files` is the user's library; `conversation_files` attaches library files
//! to conversations. Before a sandbox run, a conversation's attached files are
//! materialized into the box's `/app` directory, and files the run drops into
//! `/app/output` are saved back into the library and attached to the
//! conversation. Both the ephemeral `sandboxd` path and the persistent computer
//! path share this code.

use std::sync::Arc;

use base64::Engine as _;
use chat_providers::ImagePart;
use chat_sandbox::{RejectedFile, SandboxFile};
use chat_store::{FileRecord, FileStore, Store};
use uuid::Uuid;

use crate::error::ApiError;
use crate::file_policy::FilePolicy;
use crate::ocr::Ocr;

/// Reads attached files out of storage and writes sandbox outputs back in.
pub struct FileWorkspace {
    files: Arc<dyn FileStore>,
    store: Arc<dyn Store>,
    policy: FilePolicy,
}

/// What a conversation's attached files contribute to a turn: the text
/// preamble, and the images to forward to a vision-capable model.
#[derive(Debug, Default)]
pub struct AttachmentContext {
    pub text: Option<String>,
    pub images: Vec<ImagePart>,
}

/// The result of persisting a run's outputs: the library records created, plus
/// any outputs rejected by the size/type policy.
#[derive(Debug, Default)]
pub struct PersistedOutputs {
    pub saved: Vec<FileRecord>,
    pub rejected: Vec<RejectedFile>,
}

impl FileWorkspace {
    pub fn new(files: Arc<dyn FileStore>, store: Arc<dyn Store>, policy: FilePolicy) -> Self {
        Self {
            files,
            store,
            policy,
        }
    }

    /// Load every file attached to `conversation_id` as sandbox inputs. Missing
    /// blobs are skipped with a warning rather than failing the run.
    pub async fn load_for_conversation(
        &self,
        user_id: Uuid,
        conversation_id: Uuid,
    ) -> Result<Vec<SandboxFile>, ApiError> {
        let records = self.store.list_files(conversation_id).await?;
        let mut inputs: Vec<SandboxFile> = Vec::new();
        for record in records {
            if record.user_id != user_id {
                continue;
            }
            match self.files.get(&record.storage_path).await {
                Ok(bytes) => {
                    let name = record.filename.clone();
                    if inputs.iter().any(|f| f.name == name) {
                        tracing::warn!(file = %record.id, name, "duplicate sandbox input name; skipping");
                        continue;
                    }
                    inputs.push(SandboxFile {
                        name,
                        content_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    });
                }
                Err(err) => {
                    tracing::warn!(error = %err, file = %record.id, "failed to load sandbox input");
                }
            }
        }
        Ok(inputs)
    }

    /// Load the conversation's attached files as prompt context.
    ///
    /// Text files are decoded directly, common document formats (PDF, DOCX,
    /// XLSX, PPTX, HTML) are parsed to text, and image-only documents are
    /// recognized with `ocr` when it is configured. Each file is truncated to
    /// keep the prompt bounded; unsupported formats are skipped.
    ///
    /// With `vision`, images are returned as image parts for a vision-capable
    /// model instead of being read as text, so the model sees the pixels. An
    /// image that cannot be forwarded (too large, or past the count limit)
    /// falls back to the text path.
    pub async fn attachment_context(
        &self,
        user_id: Uuid,
        conversation_id: Uuid,
        ocr: &Ocr,
        vision: bool,
    ) -> Result<AttachmentContext, ApiError> {
        const MAX_FILE_BYTES: usize = 128 * 1024;
        const MAX_TOTAL_CHARS: usize = 256 * 1024;
        const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
        const MAX_IMAGES: usize = 4;

        let records = self.store.list_files(conversation_id).await?;
        let mut out = String::new();
        let mut images: Vec<ImagePart> = Vec::new();
        let mut included = 0usize;
        for record in records {
            if record.user_id != user_id {
                continue;
            }
            let bytes = match self.files.get(&record.storage_path).await {
                Ok(bytes) => bytes,
                Err(err) => {
                    tracing::warn!(error = %err, file = %record.id, "failed to load attachment");
                    continue;
                }
            };
            if vision && images.len() < MAX_IMAGES && bytes.len() <= MAX_IMAGE_BYTES {
                if let Some(mime) = record
                    .mime
                    .clone()
                    .filter(|mime| mime.starts_with("image/"))
                {
                    images.push(ImagePart {
                        mime,
                        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    });
                    continue;
                }
            }
            let Some(text) = extract_attachment_text(&record.filename, bytes, ocr).await else {
                continue; // unsupported or unreadable format
            };

            if included == 0 {
                out.push_str(
                    "The user has attached the following documents to this conversation. \
                     Use them to answer and say when the answer is not present.\n",
                );
            }

            let mut end = text.len().min(MAX_FILE_BYTES);
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            let truncated = text.len() > end;
            out.push_str(&format!(
                "\n--- File: {}{} ---\n",
                record.filename,
                if truncated { " (truncated)" } else { "" }
            ));
            out.push_str(&text[..end]);
            out.push('\n');
            included += 1;

            if out.len() >= MAX_TOTAL_CHARS {
                out.push_str("\n[additional attachments omitted]\n");
                break;
            }
        }
        Ok(AttachmentContext {
            text: (included > 0).then_some(out),
            images,
        })
    }

    /// Persist sandbox output files into the user's library and attach them to
    /// the conversation. Outputs whose bytes match an already-attached file are
    /// skipped, so a re-run that produces identical files is a no-op. Outputs
    /// that fail the size/type policy are returned as rejections rather than
    /// silently dropped.
    pub async fn persist_outputs(
        &self,
        user_id: Uuid,
        conversation_id: Uuid,
        outputs: &[SandboxFile],
    ) -> Result<PersistedOutputs, ApiError> {
        let mut outcome = PersistedOutputs::default();
        if outputs.is_empty() {
            return Ok(outcome);
        }
        let attached = self.store.list_files(conversation_id).await?;

        for output in outputs {
            let bytes = match base64::engine::general_purpose::STANDARD.decode(&output.content_b64)
            {
                Ok(bytes) => bytes,
                Err(err) => {
                    tracing::warn!(error = %err, name = %output.name, "invalid output encoding");
                    outcome.rejected.push(RejectedFile {
                        name: output.name.clone(),
                        reason: "invalid base64 encoding".into(),
                    });
                    continue;
                }
            };
            let filename = sanitize_filename(&output.name);
            let mime = match self.policy.check(&filename, &bytes) {
                Ok(mime) => mime,
                Err(reason) => {
                    tracing::warn!(name = %filename, reason, "rejected sandbox output");
                    outcome.rejected.push(RejectedFile {
                        name: filename,
                        reason,
                    });
                    continue;
                }
            };

            if let Some(existing) = attached.iter().find(|f| f.filename == filename) {
                match self.files.get(&existing.storage_path).await {
                    Ok(current) if current == bytes => continue,
                    _ => {}
                }
            }

            let file_id = Uuid::new_v4();
            let key = format!("{}/{}", user_id, file_id);
            let location = self.files.put(&key, &bytes).await?;
            let record = self
                .store
                .create_file(
                    user_id,
                    None,
                    &filename,
                    Some(&mime),
                    bytes.len() as i64,
                    &location,
                )
                .await?;
            self.store
                .attach_file(record.id, conversation_id, user_id)
                .await?;
            outcome.saved.push(record);
        }
        Ok(outcome)
    }

    /// Remove a library file's blob. Used when deleting a file; the row itself
    /// is removed by the caller through the store.
    pub async fn delete_blob(&self, file: &FileRecord) {
        if let Err(err) = self.files.delete(&file.storage_path).await {
            tracing::warn!(error = %err, file = %file.id, "failed to delete stored blob");
        }
    }
}

/// Decode a file to text for inlining: try text codecs first (cheap), then fall
/// back to a size-capped document parser on a blocking thread so a large PDF or
/// office file never stalls the async runtime, and finally recognize documents
/// that carry no text layer at all.
async fn extract_attachment_text(filename: &str, bytes: Vec<u8>, ocr: &Ocr) -> Option<String> {
    if let Some(text) = crate::text::decode_text(&bytes) {
        return Some(text);
    }
    if bytes.len() > crate::doctext::MAX_PARSE_BYTES {
        tracing::warn!(file = %filename, bytes = bytes.len(), "attachment too large to parse");
        return None;
    }
    let name = filename.to_string();
    let parse_bytes = bytes.clone();
    let task =
        tokio::task::spawn_blocking(move || crate::doctext::extract_text(&name, &parse_bytes));
    let parsed = match tokio::time::timeout(std::time::Duration::from_secs(20), task).await {
        Ok(Ok(text)) => text,
        Ok(Err(err)) => {
            tracing::warn!(error = %err, file = %filename, "document parse failed");
            None
        }
        Err(_) => {
            tracing::warn!(file = %filename, "document parse timed out");
            None
        }
    };
    match parsed {
        Some(text) => Some(text),
        None if ocr.enabled() && Ocr::recognizes(filename) => ocr.recognize(filename, &bytes).await,
        None => None,
    }
}

/// Keep only characters that are safe in a file name.
pub(crate) fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        "file".into()
    } else {
        cleaned
    }
}
