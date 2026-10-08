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
    /// Lifetime of an access token (short-lived; refreshed via `/api/auth/refresh`).
    pub jwt_ttl_seconds: i64,
    /// Lifetime of a refresh token. Rotated on every use.
    pub refresh_ttl_seconds: i64,
    pub log_level: String,

    /// Origins allowed to call the API cross-origin. `*` opts back into a
    /// permissive policy; an empty list denies all cross-origin requests (the
    /// API is bearer-token based and normally same-origin behind a proxy).
    pub cors_allowed_origins: Vec<String>,
    /// Whether the in-memory per-IP rate limiter is active.
    pub rate_limit_enabled: bool,
    /// Max `/api/auth/*` requests per IP per minute.
    pub rate_limit_auth_per_minute: u32,
    /// Max `/api/chat` requests per IP per minute.
    pub rate_limit_chat_per_minute: u32,

    /// Base64-encoded 256-bit key used to encrypt provider API keys at rest.
    /// When unset, a key is derived from `jwt_secret` (with a warning); set this
    /// explicitly in production so rotating `JWT_SECRET` does not strand data.
    pub secret_encryption_key: Option<String>,

    /// OpenAI-compatible endpoint used for RAG embeddings. Chat providers are
    /// configured in the database, not from the environment.
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

    /// Expose the sandbox as the `execute_code` tool to the agent runtime.
    /// Off by default so a plain deployment never grants model-driven code
    /// execution.
    pub sandbox_tool_enabled: bool,
    /// Maximum tool iterations the agent loop may run per chat turn.
    pub agent_max_iterations: usize,
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

/// Parse a comma-separated env var into a trimmed, non-empty list.
fn var_list(name: &str) -> Vec<String> {
    var(name)
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Reject obviously weak `JWT_SECRET` values at boot. A short or guessable
/// signing key lets anyone mint tokens, so this fails closed with a clear
/// message instead of silently accepting `change-me`.
fn validate_jwt_secret(secret: &str) -> Result<(), ChatError> {
    const MIN_LEN: usize = 32;
    const WEAK: &[&str] = &[
        "change-me",
        "change-me-in-production",
        "changeme",
        "secret",
        "password",
        "jwt-secret",
        "test",
        "dev",
    ];
    if secret.len() < MIN_LEN {
        return Err(ChatError::Config(format!(
            "JWT_SECRET must be at least {MIN_LEN} characters (got {}); \
             generate one with `openssl rand -base64 48`",
            secret.len()
        )));
    }
    let lowered = secret.to_ascii_lowercase();
    if WEAK.contains(&lowered.as_str()) {
        return Err(ChatError::Config(
            "JWT_SECRET is a well-known placeholder; generate one with \
             `openssl rand -base64 48`"
                .into(),
        ));
    }
    Ok(())
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

        let jwt_secret =
            var("JWT_SECRET").ok_or_else(|| ChatError::Config("JWT_SECRET is required".into()))?;
        validate_jwt_secret(&jwt_secret)?;

        Ok(Self {
            database_backend,
            database_url,
            bind_addr: var("BIND_ADDR").unwrap_or_else(|| "0.0.0.0:3080".into()),
            jwt_secret,
            jwt_ttl_seconds: var("JWT_TTL_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
            refresh_ttl_seconds: var("REFRESH_TTL_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(2_592_000),
            log_level: var("LOG_LEVEL").unwrap_or_else(|| "info".into()),

            cors_allowed_origins: match env::var("CORS_ALLOWED_ORIGINS") {
                // Explicitly set (including to empty, which denies cross-origin).
                Ok(_) => var_list("CORS_ALLOWED_ORIGINS"),
                // Sensible dev defaults: the SvelteKit dev/preview ports and the
                // API itself. Set CORS_ALLOWED_ORIGINS (or `*`) in production.
                Err(_) => vec![
                    "http://localhost:5173".into(),
                    "http://localhost:4173".into(),
                    "http://localhost:3080".into(),
                ],
            },
            rate_limit_enabled: var_bool("RATE_LIMIT_ENABLED", true),
            rate_limit_auth_per_minute: var("RATE_LIMIT_AUTH_PER_MINUTE")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            rate_limit_chat_per_minute: var("RATE_LIMIT_CHAT_PER_MINUTE")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),

            secret_encryption_key: var("SECRET_ENCRYPTION_KEY"),

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

            sandbox_tool_enabled: var_bool("SANDBOX_TOOL_ENABLED", false),
            agent_max_iterations: var("AGENT_MAX_ITERATIONS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(6),
        })
    }

    /// Build the at-rest secret cipher from `SECRET_ENCRYPTION_KEY`, falling
    /// back to a key derived from `JWT_SECRET` when unset.
    pub fn secret_cipher(&self) -> crate::Result<crate::crypto::SecretCipher> {
        match self.secret_encryption_key.as_deref() {
            Some(key) => crate::crypto::SecretCipher::from_base64(key),
            None => crate::crypto::SecretCipher::derive_from_secret(&self.jwt_secret),
        }
    }
}
