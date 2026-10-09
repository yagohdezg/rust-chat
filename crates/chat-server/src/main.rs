mod error;
mod extract;
mod rate_limit;
mod routes;
mod state;
mod stream;
mod tools;
mod workspace;

use std::sync::Arc;

use axum::http::{header, HeaderValue, Method};
use axum::middleware;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use chat_agents::{CodeInterpreterTool, ToolRegistry};
use chat_computers::{ComputerOrchestrator, HttpComputerNode, NodeRouter};
use chat_core::{Config, DatabaseBackend, FileStorageKind, SecretCipher};
use chat_db_postgres::{PgVectorStore, PostgresStore};
use chat_db_sqlite::{SqliteStore, SqliteVectorStore};
use chat_files_s3::S3FileStore;
use chat_rag::{OpenAiEmbedder, RagPipeline};
use chat_sandbox::{HttpSandboxBackend, SandboxBackend};
use chat_store::{FileStore, LocalFileStore, Store, VectorStore};

use crate::state::AppState;
use crate::stream::StreamHub;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::from_env()?;
    init_tracing(&cfg.log_level);

    // At-rest encryption for provider API keys. A dedicated key is preferred;
    // otherwise a stable key is derived from JWT_SECRET.
    if cfg.secret_encryption_key.is_none() {
        tracing::warn!(
            "SECRET_ENCRYPTION_KEY is not set; deriving provider-key encryption from JWT_SECRET \
             (set a dedicated key in production)"
        );
    }
    let secrets: Arc<SecretCipher> = Arc::new(cfg.secret_cipher()?);

    // `chat-server migrate` applies migrations and exits — intended for a
    // one-shot Kubernetes Job / initContainer so booting replicas do not race.
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        let _ = connect_store(&cfg, secrets, true).await?;
        tracing::info!("migrations applied");
        return Ok(());
    }

    // Composition root: pick the relational + vector backend from config.
    let (store, vectors) = connect_store(&cfg, secrets, cfg.migrate_on_boot).await?;
    if !cfg.migrate_on_boot {
        tracing::warn!("MIGRATE_ON_BOOT=false; expecting migrations to run out-of-band");
    }
    tracing::info!(backend = ?cfg.database_backend, "relational store ready");

    // Uploaded-file storage: local disk (dev/single replica) or S3.
    let files: Arc<dyn FileStore> = match cfg.file_storage_kind {
        FileStorageKind::Local => Arc::new(LocalFileStore::new(&cfg.file_storage_dir)),
        FileStorageKind::S3 => Arc::new(S3FileStore::new(
            cfg.s3_bucket
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("S3_BUCKET is required for FILE_STORAGE_KIND=s3"))?,
            &cfg.s3_region,
            cfg.s3_endpoint.as_deref(),
            cfg.s3_prefix.as_deref(),
        )?),
    };
    tracing::info!(kind = ?cfg.file_storage_kind, "file storage ready");

    // RAG is enabled only when an embedding key is available.
    let rag = cfg
        .openai_api_key
        .as_ref()
        .filter(|key| !key.trim().is_empty())
        .map(|key| {
            let embedder = Arc::new(OpenAiEmbedder::new(
                &cfg.openai_base_url,
                key,
                &cfg.embedding_model,
                1536,
            ));
            tracing::info!(model = %cfg.embedding_model, "RAG enabled");
            RagPipeline::new(
                vectors.clone(),
                embedder,
                cfg.rag_top_k,
                cfg.rag_chunk_chars,
                cfg.rag_chunk_overlap,
            )
        });
    if rag.is_none() {
        tracing::warn!("OPENAI_API_KEY not set; RAG/file indexing disabled");
    }

    // Execution always goes through the standalone `sandboxd` service; the
    // sandbox spec (image, limits) lives there.
    let sandbox: Arc<dyn SandboxBackend> = Arc::new(HttpSandboxBackend::new(
        cfg.sandboxd_url.clone(),
        cfg.sandboxd_token.clone(),
    ));
    tracing::info!(
        backend = sandbox.name(),
        isolation = ?sandbox.isolation(),
        url = %cfg.sandboxd_url,
        "sandbox backend ready"
    );

    // Per-user persistent "computer" control plane (§4b). Off by default; when
    // enabled it places a long-lived box per user, routes executions to its
    // node, and reaps idle computers in the background.
    let computers = if cfg.computers_enabled {
        let mut router = NodeRouter::new();
        for node in &cfg.sandbox_nodes {
            router = router.with_node(Arc::new(HttpComputerNode::new(
                node.name.clone(),
                node.url.clone(),
                cfg.sandboxd_token.clone(),
            )));
        }
        let orchestrator = Arc::new(ComputerOrchestrator::new(
            store.clone(),
            router,
            std::time::Duration::from_secs(cfg.computer_idle_ttl_seconds.max(1)),
            cfg.computer_warm_pool,
        ));
        spawn_computer_reaper(
            orchestrator.clone(),
            std::time::Duration::from_secs(cfg.computer_reap_interval_seconds.max(1)),
        );
        tracing::info!(
            nodes = cfg.sandbox_nodes.len(),
            idle_ttl_secs = cfg.computer_idle_ttl_seconds,
            warm_pool = cfg.computer_warm_pool,
            "computer control plane enabled"
        );
        Some(orchestrator)
    } else {
        None
    };

    // Tool registry for the agent runtime. The sandbox-backed code interpreter
    // is opt-in so a plain deployment never grants model-driven execution.
    let mut tools = ToolRegistry::new();
    if cfg.sandbox_tool_enabled {
        tools.register(Arc::new(CodeInterpreterTool::new(sandbox.clone())));
        tracing::info!(tool = "execute_code", "sandbox tool registered");
    }
    if !tools.is_empty() {
        tracing::info!("agent runtime enabled (tools registered)");
    }

    let bind_addr = cfg.bind_addr.clone();
    let workspace = Arc::new(workspace::FileWorkspace::new(
        files.clone(),
        store.clone(),
        vectors.clone(),
    ));
    let state = Arc::new(AppState {
        cfg: Arc::new(cfg),
        store,
        files,
        workspace,
        sandbox,
        computers,
        tools,
        rag,
        hub: Arc::new(StreamHub::new()),
        rate_limit: Arc::new(rate_limit::RateLimiter::new()),
    });

    // Auth and chat get their own per-IP rate limiters; everything else is
    // unlimited at this layer.
    let auth_routes = Router::new()
        .route("/api/auth/register", post(routes::register))
        .route("/api/auth/login", post(routes::login))
        .route("/api/auth/refresh", post(routes::refresh))
        .route("/api/auth/logout", post(routes::logout))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::limit_auth,
        ));
    let chat_routes = Router::new()
        .route("/api/chat", post(routes::chat))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::limit_chat,
        ));

    let app = Router::new()
        .route("/health", get(routes::health))
        .route("/api/me", get(routes::me))
        .route(
            "/api/admin/users",
            get(routes::admin_list_users).post(routes::admin_create_user),
        )
        .route(
            "/api/admin/users/{id}",
            patch(routes::admin_update_user).delete(routes::admin_delete_user),
        )
        .route("/api/admin/audit", get(routes::admin_list_audit))
        .route(
            "/api/admin/providers",
            post(routes::admin_bulk_create_providers),
        )
        .route("/api/models", get(routes::list_models))
        .route(
            "/api/conversations",
            get(routes::list_conversations).post(routes::create_conversation),
        )
        .route(
            "/api/conversations/{id}",
            delete(routes::delete_conversation).patch(routes::update_conversation),
        )
        .route(
            "/api/conversations/{id}/duplicate",
            post(routes::duplicate_conversation),
        )
        .route(
            "/api/agents",
            get(routes::list_agents).post(routes::create_agent),
        )
        .route("/api/agents/{id}", delete(routes::delete_agent))
        .route(
            "/api/providers",
            get(routes::list_providers).post(routes::create_provider),
        )
        .route(
            "/api/providers/{id}",
            delete(routes::delete_provider).patch(routes::update_provider),
        )
        .route(
            "/api/providers/{id}/models",
            get(routes::list_provider_models),
        )
        .route(
            "/api/providers/{id}/credential",
            put(routes::set_provider_credential),
        )
        .route(
            "/api/conversations/{id}/messages",
            get(routes::list_messages),
        )
        .route(
            "/api/conversations/{id}/files",
            get(routes::list_files).post(routes::attach_conversation_file),
        )
        .route(
            "/api/conversations/{id}/files/{file_id}",
            delete(routes::detach_conversation_file),
        )
        .route(
            "/api/files",
            get(routes::list_user_files).post(routes::upload_file),
        )
        .route(
            "/api/files/{id}",
            get(routes::download_file).delete(routes::delete_file),
        )
        .route("/api/messages/{id}/stream", get(routes::resume_stream))
        .route("/api/sandbox/run", post(routes::sandbox_run))
        .route(
            "/api/computers/me",
            get(routes::get_computer).delete(routes::destroy_computer),
        )
        .route("/api/computers/me/pause", post(routes::pause_computer))
        .route("/api/computers/me/resume", post(routes::resume_computer))
        .route("/api/computers/exec", post(routes::computer_exec))
        .merge(auth_routes)
        .merge(chat_routes)
        .layer(TraceLayer::new_for_http())
        .layer(build_cors(&state.cfg))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, "chat-server listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Build the CORS policy from `CORS_ALLOWED_ORIGINS`. `*` restores a permissive
/// policy; otherwise only the listed origins may call the API cross-origin.
fn build_cors(cfg: &Config) -> CorsLayer {
    let layer = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    if cfg.cors_allowed_origins.iter().any(|origin| origin == "*") {
        layer.allow_origin(AllowOrigin::any())
    } else {
        let origins: Vec<HeaderValue> = cfg
            .cors_allowed_origins
            .iter()
            .filter_map(|origin| origin.parse::<HeaderValue>().ok())
            .collect();
        layer.allow_origin(AllowOrigin::list(origins))
    }
}

/// Connect to the configured relational + vector backend, optionally applying
/// migrations. Shared by the server boot path and the `migrate` subcommand.
async fn connect_store(
    cfg: &Config,
    secrets: Arc<SecretCipher>,
    migrate: bool,
) -> anyhow::Result<(Arc<dyn Store>, Arc<dyn VectorStore>)> {
    match cfg.database_backend {
        DatabaseBackend::Postgres => {
            let db = PostgresStore::connect(&cfg.database_url, secrets).await?;
            if migrate {
                db.migrate().await?;
                let rewritten = db.encrypt_plaintext_provider_keys().await?;
                if rewritten > 0 {
                    tracing::info!(rows = rewritten, "encrypted provider API keys at rest");
                }
            }
            let vectors = Arc::new(PgVectorStore::new(db.pool()));
            Ok((Arc::new(db), vectors))
        }
        DatabaseBackend::Sqlite => {
            let db = SqliteStore::connect(&cfg.database_url, secrets).await?;
            if migrate {
                db.migrate().await?;
                let rewritten = db.encrypt_plaintext_provider_keys().await?;
                if rewritten > 0 {
                    tracing::info!(rows = rewritten, "encrypted provider API keys at rest");
                }
            }
            let vectors = Arc::new(SqliteVectorStore::new(db.pool()));
            Ok((Arc::new(db), vectors))
        }
    }
}

/// Background loop that destroys idle computers and tops up the warm pool.
///
/// A single reaper is spawned per process. With several replicas each reaps
/// against the shared registry; `list_idle_computers` + state transitions keep
/// the work convergent even if two tick at once.
fn spawn_computer_reaper(orchestrator: Arc<ComputerOrchestrator>, interval: std::time::Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match orchestrator.reap_idle().await {
                Ok(0) => {}
                Ok(count) => tracing::info!(count, "reaped idle computers"),
                Err(err) => tracing::warn!(error = %err, "idle computer reaper failed"),
            }
            if let Err(err) = orchestrator.refill_warm_pool().await {
                tracing::warn!(error = %err, "warm pool refill failed");
            }
        }
    });
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}
