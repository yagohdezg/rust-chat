use std::env;

use serde::{Deserialize, Serialize};

/// Which sandbox backend to use for untrusted code execution.
///
/// `Podman` is the dev/fallback backend (shared-kernel, rootless containers).
/// `Boxlite` is the production backend: one hardware-isolated microVM per run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxBackendKind {
    #[default]
    Podman,
    Boxlite,
}

impl std::str::FromStr for SandboxBackendKind {
    type Err = ChatError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "podman" => Ok(Self::Podman),
            "boxlite" => Ok(Self::Boxlite),
            other => Err(ChatError::Config(format!(
                "unknown SANDBOX_BACKEND `{other}` (expected `podman` or `boxlite`)"
            ))),
        }
    }
}

/// Which relational backend to persist to.
///
/// The vector store is chosen to match (`pgvector` for Postgres, brute-force
/// SQLite otherwise), so switching here switches both seams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseBackend {
    #[default]
    Postgres,
    Sqlite,
}

/// Where uploaded files live.
///
/// `Local` writes to `FILE_STORAGE_DIR` on the pod's disk. `S3` writes to any
/// S3-compatible object store (AWS S3, MinIO, R2, ...), which is required for
/// correctness when more than one replica serves uploads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStorageKind {
    #[default]
    Local,
    S3,
}

impl std::str::FromStr for FileStorageKind {
    type Err = ChatError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "local" | "fs" | "disk" => Ok(Self::Local),
            "s3" | "object" => Ok(Self::S3),
            other => Err(ChatError::Config(format!(
                "unknown FILE_STORAGE_KIND `{other}` (expected `local` or `s3`)"
            ))),
        }
    }
}

impl std::str::FromStr for DatabaseBackend {
    type Err = ChatError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" => Ok(Self::Postgres),
            "sqlite" => Ok(Self::Sqlite),
            other => Err(ChatError::Config(format!(
                "unknown DB_BACKEND `{other}` (expected `postgres` or `sqlite`)"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub database_backend: DatabaseBackend,
    pub database_url: String,
    pub bind_addr: String,
    pub jwt_secret: String,
    pub jwt_ttl_seconds: i64,
    pub log_level: String,

    pub openai_api_key: Option<String>,
    pub openai_base_url: String,

    /// Whether the server applies migrations on boot. Set `false` in production
    /// and run migrations from a one-shot Job instead.
    pub migrate_on_boot: bool,

    /// Where uploaded files are stored: local disk or S3-compatible object store.
    pub file_storage_kind: FileStorageKind,
    /// Directory where uploaded files are stored when `file_storage_kind` is
    /// `local`.
    pub file_storage_dir: String,
    /// Bucket for uploaded files when `file_storage_kind` is `s3`.
    pub s3_bucket: Option<String>,
    /// Region for the S3 bucket (defaults to `us-east-1`).
    pub s3_region: String,
    /// Custom endpoint for S3-compatible stores (MinIO, R2, ...).
    pub s3_endpoint: Option<String>,
    /// Optional key prefix inside the bucket.
    pub s3_prefix: Option<String>,
    /// OpenAI-compatible embedding model used for RAG.
    pub embedding_model: String,
    /// Number of chunks to retrieve per query.
    pub rag_top_k: usize,
    /// Approximate chunk size in characters.
    pub rag_chunk_chars: usize,
    /// Chunk overlap in characters.
    pub rag_chunk_overlap: usize,

    pub sandbox_backend: SandboxBackendKind,
    pub sandbox_image: String,
    pub sandbox_timeout_seconds: u64,
    pub sandbox_memory_mb: u64,
    pub sandbox_cpus: f64,
    pub boxlite_url: String,
}

use crate::error::ChatError;

fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Parse a boolean-ish env var (`1/true/yes/on` -> true, `0/false/no/off` -> false).
fn var_bool(name: &str, default: bool) -> bool {
    match var(name) {
        Some(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => default,
    }
}

impl Config {
    /// Load configuration from the process environment (and a local `.env` if present).
    pub fn from_env() -> Result<Self, ChatError> {
        let _ = dotenvy::dotenv();

        let database_url = var("DATABASE_URL")
            .ok_or_else(|| ChatError::Config("DATABASE_URL is required".into()))?;

        let database_backend = var("DB_BACKEND")
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or_default();

        let sandbox_backend = var("SANDBOX_BACKEND")
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or_default();

        let file_storage_kind = var("FILE_STORAGE_KIND")
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or_default();

        let s3_bucket = var("S3_BUCKET");
        if file_storage_kind == FileStorageKind::S3 && s3_bucket.is_none() {
            return Err(ChatError::Config(
                "S3_BUCKET is required when FILE_STORAGE_KIND=s3".into(),
            ));
        }

        Ok(Self {
            database_backend,
            database_url,
            bind_addr: var("BIND_ADDR").unwrap_or_else(|| "0.0.0.0:3080".into()),
            jwt_secret: var("JWT_SECRET")
                .ok_or_else(|| ChatError::Config("JWT_SECRET is required".into()))?,
            jwt_ttl_seconds: var("JWT_TTL_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
            log_level: var("LOG_LEVEL").unwrap_or_else(|| "info".into()),

            openai_api_key: var("OPENAI_API_KEY"),
            openai_base_url: var("OPENAI_BASE_URL")
                .unwrap_or_else(|| "https://api.openai.com/v1".into()),

            migrate_on_boot: var_bool("MIGRATE_ON_BOOT", true),
            file_storage_kind,
            file_storage_dir: var("FILE_STORAGE_DIR").unwrap_or_else(|| "./.data/files".into()),
            s3_bucket,
            s3_region: var("S3_REGION").unwrap_or_else(|| "us-east-1".into()),
            s3_endpoint: var("S3_ENDPOINT"),
            s3_prefix: var("S3_PREFIX"),
            embedding_model: var("EMBEDDING_MODEL")
                .unwrap_or_else(|| "text-embedding-3-small".into()),
            rag_top_k: var("RAG_TOP_K").and_then(|v| v.parse().ok()).unwrap_or(4),
            rag_chunk_chars: var("RAG_CHUNK_CHARS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1200),
            rag_chunk_overlap: var("RAG_CHUNK_OVERLAP")
                .and_then(|v| v.parse().ok())
                .unwrap_or(200),

            sandbox_backend,
            sandbox_image: var("SANDBOX_IMAGE").unwrap_or_else(|| "python:3.12-slim".into()),
            sandbox_timeout_seconds: var("SANDBOX_TIMEOUT_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            sandbox_memory_mb: var("SANDBOX_MEMORY_MB")
                .and_then(|v| v.parse().ok())
                .unwrap_or(512),
            sandbox_cpus: var("SANDBOX_CPUS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0),
            boxlite_url: var("BOXLITE_URL").unwrap_or_else(|| "http://localhost:8100".into()),
        })
    }
}
