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
use chat_sandbox::SandboxFile;
use chat_store::{FileRecord, FileStore, Store, VectorStore};
use uuid::Uuid;

use crate::error::ApiError;

/// Reads attached files out of storage and writes sandbox outputs back in.
pub struct FileWorkspace {
    files: Arc<dyn FileStore>,
    store: Arc<dyn Store>,
    vectors: Arc<dyn VectorStore>,
}

impl FileWorkspace {
    pub fn new(
        files: Arc<dyn FileStore>,
        store: Arc<dyn Store>,
        vectors: Arc<dyn VectorStore>,
    ) -> Self {
        Self {
            files,
            store,
            vectors,
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

    /// Persist sandbox output files into the user's library and attach them to
    /// the conversation. Outputs whose bytes match an already-attached file are
    /// skipped, so a re-run that produces identical files is a no-op.
    pub async fn persist_outputs(
        &self,
        user_id: Uuid,
        conversation_id: Uuid,
        outputs: &[SandboxFile],
    ) -> Result<Vec<FileRecord>, ApiError> {
        if outputs.is_empty() {
            return Ok(Vec::new());
        }
        let attached = self.store.list_files(conversation_id).await?;
        let mut saved = Vec::new();

        for output in outputs {
            let bytes = match base64::engine::general_purpose::STANDARD.decode(&output.content_b64)
            {
                Ok(bytes) => bytes,
                Err(err) => {
                    tracing::warn!(error = %err, name = %output.name, "invalid output encoding");
                    continue;
                }
            };
            let filename = sanitize_filename(&output.name);

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
                    mime_for(&filename).as_deref(),
                    bytes.len() as i64,
                    &location,
                )
                .await?;
            self.store
                .attach_file(record.id, conversation_id, user_id)
                .await?;
            saved.push(record);
        }
        Ok(saved)
    }

    /// Remove a library file's blob and embeddings. Used when deleting a file;
    /// the row itself is removed by the caller through the store.
    pub async fn delete_blob(&self, file: &FileRecord) {
        if let Err(err) = self.files.delete(&file.storage_path).await {
            tracing::warn!(error = %err, file = %file.id, "failed to delete stored blob");
        }
        if let Err(err) = self.vectors.delete_for_file(file.id).await {
            tracing::warn!(error = %err, file = %file.id, "failed to delete embeddings");
        }
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

/// Best-effort MIME guess from a file extension (used for downloads).
fn mime_for(name: &str) -> Option<String> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => return None,
    };
    Some(mime.to_string())
}
