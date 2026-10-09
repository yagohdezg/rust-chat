use std::env;

use serde::{Deserialize, Serialize};

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

/// One sandbox node the computer orchestrator may place per-user boxes on.
///
/// `name` is what the placement registry stores; `url` is the `sandboxd` base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxNodeConfig {
    pub name: String,
    pub url: String,
}

/// Parse `SANDBOX_NODES` (`name=url[,name=url...]`) into node configs. An empty
/// or unset value yields a single `default` node at `default_url`.
fn parse_sandbox_nodes(
    value: Option<&str>,
    default_url: &str,
) -> Result<Vec<SandboxNodeConfig>, ChatError> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(vec![SandboxNodeConfig {
            name: "default".into(),
            url: default_url.to_string(),
        }]);
    };

    let mut nodes = Vec::new();
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (name, url) = entry.split_once('=').ok_or_else(|| {
            ChatError::Config(format!("SANDBOX_NODES entry `{entry}` must be `name=url`"))
        })?;
        let name = name.trim();
        let url = url.trim();
        if name.is_empty() || url.is_empty() {
            return Err(ChatError::Config(format!(
                "SANDBOX_NODES entry `{entry}` must be `name=url`"
            )));
        }
        nodes.push(SandboxNodeConfig {
            name: name.to_string(),
            url: url.to_string(),
        });
    }
    if nodes.is_empty() {
        return Err(ChatError::Config("SANDBOX_NODES contained no nodes".into()));
    }
    Ok(nodes)
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

    /// Base URL of the standalone `sandboxd` execution service.
    pub sandboxd_url: String,
    /// Shared bearer token presented to `sandboxd` (`SANDBOXD_TOKEN`).
    pub sandboxd_token: Option<String>,

    /// Expose the per-user persistent "computer" control plane (§4b). Off by
    /// default; requires `sandboxd` (BoxLite) nodes.
    pub computers_enabled: bool,
    /// Sandbox nodes computers may be placed on, parsed from `SANDBOX_NODES`
    /// (`name=url[,name=url...]`). When unset, a single `default` node points at
    /// `sandboxd_url`.
    pub sandbox_nodes: Vec<SandboxNodeConfig>,
    /// A running/paused computer untouched for this long is destroyed.
    pub computer_idle_ttl_seconds: u64,
    /// How often the idle reaper (and warm-pool refill) runs.
    pub computer_reap_interval_seconds: u64,
    /// Blank boxes to keep warm per node, to hide box cold-start (0 disables).
    pub computer_warm_pool: usize,

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

        let sandboxd_url = var("SANDBOXD_URL").unwrap_or_else(|| "http://localhost:3081".into());
        let sandbox_nodes = parse_sandbox_nodes(var("SANDBOX_NODES").as_deref(), &sandboxd_url)?;

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

            sandboxd_url,
            sandboxd_token: var("SANDBOXD_TOKEN"),

            computers_enabled: var_bool("COMPUTERS_ENABLED", false),
            sandbox_nodes,
            computer_idle_ttl_seconds: var("COMPUTER_IDLE_TTL_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1800),
            computer_reap_interval_seconds: var("COMPUTER_REAP_INTERVAL_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            computer_warm_pool: var("COMPUTER_WARM_POOL")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_nodes_default_to_a_single_node() {
        let nodes = parse_sandbox_nodes(None, "http://localhost:3081").unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "default");
        assert_eq!(nodes[0].url, "http://localhost:3081");

        // Blank (e.g. `${SANDBOX_NODES:-}` in compose) is treated as unset.
        let nodes = parse_sandbox_nodes(Some("   "), "http://x:3081").unwrap();
        assert_eq!(nodes[0].url, "http://x:3081");
    }

    #[test]
    fn sandbox_nodes_parse_named_pairs() {
        let nodes =
            parse_sandbox_nodes(Some("a=http://a:3081, b = http://b:3081"), "http://x").unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(
            (nodes[0].name.as_str(), nodes[0].url.as_str()),
            ("a", "http://a:3081")
        );
        assert_eq!(
            (nodes[1].name.as_str(), nodes[1].url.as_str()),
            ("b", "http://b:3081")
        );
    }

    #[test]
    fn sandbox_nodes_reject_malformed_entries() {
        assert!(parse_sandbox_nodes(Some("no-equals"), "http://x").is_err());
        assert!(parse_sandbox_nodes(Some("=http://x"), "http://x").is_err());
        assert!(parse_sandbox_nodes(Some("a="), "http://x").is_err());
    }
}
