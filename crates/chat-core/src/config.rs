use std::env;

use serde::{Deserialize, Serialize};

use crate::file_config::{FileConfig, InterfaceSection, SpeechSection};

/// Which relational backend to persist to.
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

    /// Public base URL of the web app, used to build the links sent in email.
    pub public_url: String,
    /// Sendmail-compatible command that delivers mail read from stdin (e.g.
    /// `msmtp -t` or `sendmail -t`). When unset, mail is not delivered and a
    /// warning is logged instead, so the flows stay usable without an MTA.
    pub mail_send_command: Option<String>,
    /// `From:` address on outbound mail.
    pub mail_from: String,
    /// Lifetime of an email verification link.
    pub email_verification_ttl_seconds: i64,
    /// Lifetime of a password reset link.
    pub password_reset_ttl_seconds: i64,
    /// Require a confirmed email address before an account can log in.
    pub require_email_verification: bool,
    /// Whether self-service registration is open.
    pub registration_enabled: bool,
    /// Email domains permitted to register (empty allows any domain).
    pub registration_allowed_domains: Vec<String>,
    /// Consecutive failed sign-ins that lock an account out.
    pub login_max_failures: i64,
    /// How long an account stays locked after too many failed sign-ins.
    pub login_lockout_seconds: i64,

    /// Base64-encoded 256-bit key used to encrypt provider API keys at rest.
    /// When unset, a key is derived from `jwt_secret` (with a warning); set this
    /// explicitly in production so rotating `JWT_SECRET` does not strand data.
    pub secret_encryption_key: Option<String>,

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

    /// Largest single sandbox output file that may be saved into the file
    /// library. Oversized outputs are rejected and reported to the caller
    /// instead of being silently dropped.
    pub file_output_max_bytes: u64,
    /// Allowed types for sandbox outputs, matched against the sniffed MIME type
    /// or the file extension. Entries may be a full MIME (`image/png`), a
    /// wildcard (`image/*`) or a bare extension (`png`). Empty means every type
    /// is allowed.
    pub file_output_allowed_types: Vec<String>,

    /// Largest single upload accepted, in bytes. Uploads are rejected before
    /// they are written to storage.
    pub upload_max_bytes: u64,
    /// Optional external scanner run on every upload's bytes (antivirus hook).
    /// The command receives the file on stdin; exit 0 accepts, any non-zero
    /// exit rejects the upload.
    pub upload_scan_command: Option<String>,
    /// Optional external OCR command for documents with no text layer (scans,
    /// screenshots, image-only PDFs). The command receives the document as a
    /// file path in `$1` and prints the recognized text on stdout. When unset,
    /// image-only documents are skipped.
    pub ocr_command: Option<String>,

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
    /// Ownership is durable: the pool is recorded in the store and reconciled
    /// against the nodes each reaper tick, so leaked boxes are reclaimed.
    pub computer_warm_pool: usize,
    /// Mount a durable per-user workspace volume so a computer's files survive
    /// its box being stopped, snapshotted, reaped or re-provisioned.
    pub computer_workspace: bool,

    /// Expose the sandbox as the `execute_code` tool to the agent runtime.
    /// Off by default so a plain deployment never grants model-driven code
    /// execution.
    pub sandbox_tool_enabled: bool,
    /// Maximum tool iterations the agent loop may run per chat turn.
    pub agent_max_iterations: usize,

    /// Expose user-configured MCP servers' tools to the agent runtime. Off by
    /// default: a stdio server is an arbitrary executable spawned on the host,
    /// so this is opt-in until sandboxed execution lands.
    pub mcp_enabled: bool,
    /// A connected MCP server idle for this long is shut down (its process
    /// killed) and reconnected on demand.
    pub mcp_idle_ttl_seconds: u64,

    /// Models every caller may use, as a deployment-wide allow-list. Empty means
    /// no global cap; a provider's and a user's own allow-lists still apply.
    pub model_allowlist: Vec<String>,

    /// UI copy and links declared in `rustchat.yaml` (no consumer yet).
    pub interface: InterfaceSection,
    /// Speech feature toggle declared in `rustchat.yaml` (no consumer yet).
    pub speech: SpeechSection,

    /// Parsed `rustchat.yaml` (path from `RUSTCHAT_CONFIG`). This is the base
    /// layer: environment variables and runtime/DB state override it.
    pub file: FileConfig,
}

use crate::error::ChatError;

fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Parse a boolean-ish env var (`1/true/yes/on` -> true, `0/false/no/off` -> false).
fn var_bool(name: &str, default: bool) -> bool {
    layered_bool(name, None, default)
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

/// Resolve a layered setting: a set environment variable wins, then the
/// `rustchat.yaml` value, then the built-in default.
fn layered<T: std::str::FromStr>(env_name: &str, file: Option<T>, default: T) -> T {
    var(env_name)
        .and_then(|value| value.parse().ok())
        .or(file)
        .unwrap_or(default)
}

/// Like [`layered`], for boolean-ish values (`1/true/yes/on` -> true).
fn layered_bool(env_name: &str, file: Option<bool>, default: bool) -> bool {
    match var(env_name) {
        Some(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => file.unwrap_or(default),
    }
}

/// Like [`layered`], for comma-separated lists.
fn layered_list(env_name: &str, file: Option<Vec<String>>, default: Vec<String>) -> Vec<String> {
    match var(env_name) {
        Some(_) => var_list(env_name),
        None => file.unwrap_or(default),
    }
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

        // Optional declarative base layer; env/runtime state override it.
        let file = FileConfig::load()?;

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
            rate_limit_enabled: layered_bool("RATE_LIMIT_ENABLED", file.rate_limits.enabled, true),
            rate_limit_auth_per_minute: layered(
                "RATE_LIMIT_AUTH_PER_MINUTE",
                file.rate_limits.auth_per_minute,
                30,
            ),
            rate_limit_chat_per_minute: layered(
                "RATE_LIMIT_CHAT_PER_MINUTE",
                file.rate_limits.chat_per_minute,
                60,
            ),

            public_url: var("PUBLIC_URL")
                .unwrap_or_else(|| "http://localhost:5173".into())
                .trim_end_matches('/')
                .to_string(),
            mail_send_command: var("MAIL_SEND_COMMAND"),
            mail_from: var("MAIL_FROM").unwrap_or_else(|| "rust-chat <no-reply@localhost>".into()),
            email_verification_ttl_seconds: var("EMAIL_VERIFICATION_TTL_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(86_400),
            password_reset_ttl_seconds: var("PASSWORD_RESET_TTL_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3_600),
            require_email_verification: var_bool("REQUIRE_EMAIL_VERIFICATION", false),
            registration_enabled: layered_bool(
                "REGISTRATION_ENABLED",
                file.registration.enabled,
                true,
            ),
            registration_allowed_domains: layered_list(
                "REGISTRATION_ALLOWED_DOMAINS",
                Some(file.registration.allowed_domains.clone()),
                Vec::new(),
            ),
            login_max_failures: var("LOGIN_MAX_FAILURES")
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            login_lockout_seconds: var("LOGIN_LOCKOUT_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(900),

            secret_encryption_key: var("SECRET_ENCRYPTION_KEY"),

            migrate_on_boot: var_bool("MIGRATE_ON_BOOT", true),
            file_storage_kind,
            file_storage_dir: var("FILE_STORAGE_DIR").unwrap_or_else(|| "./.data/files".into()),
            s3_bucket,
            s3_region: var("S3_REGION").unwrap_or_else(|| "us-east-1".into()),
            s3_endpoint: var("S3_ENDPOINT"),
            s3_prefix: var("S3_PREFIX"),

            file_output_max_bytes: layered(
                "FILE_OUTPUT_MAX_BYTES",
                file.file.output_max_bytes,
                8 * 1024 * 1024,
            ),
            file_output_allowed_types: layered_list(
                "FILE_OUTPUT_ALLOWED_TYPES",
                file.file.output_allowed_types.clone(),
                Vec::new(),
            ),
            upload_max_bytes: layered("UPLOAD_MAX_BYTES", file.file.max_bytes, 25 * 1024 * 1024),
            upload_scan_command: var("UPLOAD_SCAN_COMMAND"),
            ocr_command: var("OCR_COMMAND"),

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
            computer_workspace: var_bool("COMPUTER_WORKSPACE", true),

            sandbox_tool_enabled: var_bool("SANDBOX_TOOL_ENABLED", false),
            agent_max_iterations: var("AGENT_MAX_ITERATIONS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(6),

            mcp_enabled: var_bool("MCP_ENABLED", false),
            mcp_idle_ttl_seconds: var("MCP_IDLE_TTL_SECONDS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            model_allowlist: var_list("MODEL_ALLOWLIST"),

            interface: file.interface.clone(),
            speech: file.speech.clone(),

            file,
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
