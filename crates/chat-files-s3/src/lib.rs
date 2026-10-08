//! S3-compatible [`FileStore`] backend.
//!
//! Works with AWS S3 and any S3-compatible object store (MinIO, Cloudflare R2,
//! Backblaze B2, ...). Credentials come from the standard AWS environment
//! variables / instance profile via `rust-s3`'s default credentials chain.

use chat_core::{ChatError, Result};
use chat_store::FileStore;
use s3::{creds::Credentials, Bucket, Region};

/// Uploads stored in a bucket under an optional key prefix.
pub struct S3FileStore {
    bucket: Box<Bucket>,
    prefix: String,
}

impl S3FileStore {
    /// Build a store for `bucket` in `region`. Pass `endpoint` for non-AWS
    /// stores (MinIO/R2) and `prefix` to namespace keys inside the bucket.
    pub fn new(
        bucket: &str,
        region: &str,
        endpoint: Option<&str>,
        prefix: Option<&str>,
    ) -> Result<Self> {
        let region = match endpoint {
            Some(endpoint) => Region::Custom {
                region: region.to_string(),
                endpoint: endpoint.trim_end_matches('/').to_string(),
            },
            None => region
                .parse()
                .map_err(|e| ChatError::Config(format!("invalid S3 region `{region}`: {e}")))?,
        };

        let credentials = Credentials::default()
            .map_err(|e| ChatError::Config(format!("could not resolve S3 credentials: {e}")))?;

        // Path-style addressing is required by MinIO and most self-hosted
        // stores; AWS S3 accepts it too.
        let bucket = Bucket::new(bucket, region, credentials)
            .map_err(|e| ChatError::Config(format!("invalid S3 bucket `{bucket}`: {e}")))?
            .with_path_style();

        let prefix = prefix
            .map(|p| p.trim_matches('/'))
            .filter(|p| !p.is_empty())
            .map(|p| format!("{p}/"))
            .unwrap_or_default();

        Ok(Self { bucket, prefix })
    }

    fn object_key(&self, location: &str) -> String {
        format!("{}{}", self.prefix, location.trim_start_matches('/'))
    }
}

#[async_trait::async_trait]
impl FileStore for S3FileStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<String> {
        let path = self.object_key(key);
        self.bucket
            .put_object(&path, bytes)
            .await
            .map_err(|e| ChatError::Internal(anyhow::anyhow!("S3 put `{path}` failed: {e}")))?;
        Ok(path)
    }

    async fn get(&self, location: &str) -> Result<Vec<u8>> {
        let response = self
            .bucket
            .get_object(location)
            .await
            .map_err(|e| match e {
                s3::error::S3Error::HttpFailWithBody(404, _) => ChatError::NotFound,
                other => {
                    ChatError::Internal(anyhow::anyhow!("S3 get `{location}` failed: {other}"))
                }
            })?;
        Ok(response.bytes().to_vec())
    }

    async fn delete(&self, location: &str) -> Result<()> {
        self.bucket.delete_object(location).await.map_err(|e| {
            ChatError::Internal(anyhow::anyhow!("S3 delete `{location}` failed: {e}"))
        })?;
        Ok(())
    }
}
