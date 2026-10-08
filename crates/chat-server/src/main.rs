mod error;
mod extract;
mod routes;
mod state;
mod stream;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use chat_core::{Config, DatabaseBackend, FileStorageKind, SandboxBackendKind};
use chat_db_postgres::{PgVectorStore, PostgresStore};
use chat_db_sqlite::{SqliteStore, SqliteVectorStore};
use chat_files_s3::S3FileStore;
use chat_providers::OpenAiProvider;
use chat_rag::{OpenAiEmbedder, RagPipeline};
use chat_sandbox::{BoxliteBackend, PodmanBackend, SandboxBackend};
use chat_store::{FileStore, LocalFileStore, Store, VectorStore};

use crate::state::AppState;
use crate::stream::StreamHub;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::from_env()?;
    init_tracing(&cfg.log_level);

    // `chat-server migrate` applies migrations and exits — intended for a
    // one-shot Kubernetes Job / initContainer so booting replicas do not race.
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        let _ = connect_store(&cfg, true).await?;
        tracing::info!("migrations applied");
        return Ok(());
    }

    // Composition root: pick the relational + vector backend from config.
    let (store, vectors) = connect_store(&cfg, cfg.migrate_on_boot).await?;
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

    let provider = Arc::new(OpenAiProvider::new(
        &cfg.openai_base_url,
        cfg.openai_api_key.clone().unwrap_or_default(),
    ));

    let sandbox: Arc<dyn SandboxBackend> = match cfg.sandbox_backend {
        SandboxBackendKind::Podman => Arc::new(PodmanBackend::new()),
        SandboxBackendKind::Boxlite => Arc::new(BoxliteBackend::new(&cfg.boxlite_url)),
    };
    tracing::info!(
        backend = sandbox.name(),
        isolation = ?sandbox.isolation(),
        "sandbox backend ready"
    );

    let bind_addr = cfg.bind_addr.clone();
    let state = Arc::new(AppState {
        cfg: Arc::new(cfg),
        store,
        vectors,
        files,
        provider,
        sandbox,
        rag,
        hub: Arc::new(StreamHub::new()),
    });

    let app = Router::new()
        .route("/health", get(routes::health))
        .route("/api/auth/register", post(routes::register))
        .route("/api/auth/login", post(routes::login))
        .route("/api/me", get(routes::me))
        .route(
            "/api/conversations",
            get(routes::list_conversations).post(routes::create_conversation),
        )
        .route(
            "/api/conversations/{id}/messages",
            get(routes::list_messages),
        )
        .route("/api/conversations/{id}/files", get(routes::list_files))
        .route("/api/files", post(routes::upload_file))
        .route(
            "/api/files/{id}",
            get(routes::download_file).delete(routes::delete_file),
        )
        .route("/api/messages/{id}/stream", get(routes::resume_stream))
        .route("/api/chat", post(routes::chat))
        .route("/api/sandbox/run", post(routes::sandbox_run))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, "chat-server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Connect to the configured relational + vector backend, optionally applying
/// migrations. Shared by the server boot path and the `migrate` subcommand.
async fn connect_store(
    cfg: &Config,
    migrate: bool,
) -> anyhow::Result<(Arc<dyn Store>, Arc<dyn VectorStore>)> {
    match cfg.database_backend {
        DatabaseBackend::Postgres => {
            let db = PostgresStore::connect(&cfg.database_url).await?;
            if migrate {
                db.migrate().await?;
            }
            let vectors = Arc::new(PgVectorStore::new(db.pool()));
            Ok((Arc::new(db), vectors))
        }
        DatabaseBackend::Sqlite => {
            let db = SqliteStore::connect(&cfg.database_url).await?;
            if migrate {
                db.migrate().await?;
            }
            let vectors = Arc::new(SqliteVectorStore::new(db.pool()));
            Ok((Arc::new(db), vectors))
        }
    }
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}
