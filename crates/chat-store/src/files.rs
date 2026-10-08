//! Object storage for uploaded files.
//!
//! [`FileStore`] abstracts *where* file bytes live, independent of the
//! relational store. Two implementations are provided: [`LocalFileStore`]
//! (a directory on the pod's disk) and, in the sibling `chat-files-s3` crate,
//! `S3FileStore` for any S3-compatible object store.
//!
//! The relational `files` table keeps the opaque `location` string returned by
//! [`FileStore::put`]; callers never interpret it.

use std::path::PathBuf;

use chat_core::Result;

/// Content-addressed-ish blob storage for uploads.
///
/// Callers supply a stable `key` (e.g. `<user_id>/<file_id>-name.txt`) and the
/// backend returns an opaque `location` to persist alongside the file record.
#[async_trait::async_trait]
pub trait FileStore: Send + Sync {
    /// Store `bytes` under `key`, returning the canonical location.
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<String>;

    /// Fetch the bytes previously stored at `location`.
    async fn get(&self, location: &str) -> Result<Vec<u8>>;

    /// Remove the object at `location`. Idempotent: missing objects are not an
    /// error.
    async fn delete(&self, location: &str) -> Result<()>;
}

/// Filesystem-backed [`FileStore`].
///
/// Suitable for a single replica or a shared volume (EFS/NFS). Not correct
/// behind a load balancer with per-pod local disks — use S3 there.
#[derive(Debug, Clone)]
pub struct LocalFileStore {
    root: PathBuf,
}

impl LocalFileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Turn an opaque location (a key relative to the root) into a safe path.
    /// Absolute roots and `..` components are ignored so a crafted location
    /// cannot escape the configured root.
    fn resolve(&self, location: &str) -> PathBuf {
        let mut path = self.root.clone();
        let relative = location.trim_start_matches(['/', '\\']);
        for component in std::path::Path::new(relative).components() {
            if let std::path::Component::Normal(part) = component {
                path.push(part);
            }
        }
        path
    }
}

#[async_trait::async_trait]
impl FileStore for LocalFileStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<String> {
        let location = key.trim_start_matches(['/', '\\']).to_string();
        let path = self.resolve(&location);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                chat_core::ChatError::Internal(anyhow::anyhow!(
                    "failed to create storage dir {}: {e}",
                    parent.display()
                ))
            })?;
        }
        tokio::fs::write(&path, bytes).await.map_err(|e| {
            chat_core::ChatError::Internal(anyhow::anyhow!(
                "failed to write {}: {e}",
                path.display()
            ))
        })?;
        // The location is the key relative to the root, so it stays valid if the
        // root moves (e.g. a different mount point) and never leaks host paths.
        Ok(location)
    }

    async fn get(&self, location: &str) -> Result<Vec<u8>> {
        let path = self.resolve(location);
        tokio::fs::read(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                chat_core::ChatError::NotFound
            } else {
                chat_core::ChatError::Internal(anyhow::anyhow!(
                    "failed to read {}: {e}",
                    path.display()
                ))
            }
        })
    }

    async fn delete(&self, location: &str) -> Result<()> {
        let path = self.resolve(location);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(chat_core::ChatError::Internal(anyhow::anyhow!(
                "failed to delete {}: {e}",
                path.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_store_roundtrip_and_idempotent_delete() {
        let dir = std::env::temp_dir().join(format!("rustchat-fs-{}", uuid::Uuid::new_v4()));
        let store = LocalFileStore::new(&dir);

        let location = store.put("user/file.txt", b"hello").await.unwrap();
        assert_eq!(store.get(&location).await.unwrap(), b"hello");

        store.delete(&location).await.unwrap();
        assert!(matches!(
            store.get(&location).await,
            Err(chat_core::ChatError::NotFound)
        ));
        // Deleting a missing object is not an error.
        store.delete(&location).await.unwrap();

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
