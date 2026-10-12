use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Result};
use testcontainers_modules::testcontainers::core::WaitFor;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, Image};

use super::fake_provider::{FakeProvider, Script};
use super::fake_sandbox::FakeSandbox;

#[derive(Debug, Default, Clone)]
struct PgVector;

impl Image for PgVector {
    fn name(&self) -> &str {
        "pgvector/pgvector"
    }

    fn tag(&self) -> &str {
        "pg16"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![
            WaitFor::message_on_stderr("database system is ready to accept connections"),
            WaitFor::message_on_stdout("database system is ready to accept connections"),
        ]
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        [
            ("POSTGRES_DB", "postgres"),
            ("POSTGRES_USER", "postgres"),
            ("POSTGRES_PASSWORD", "postgres"),
        ]
    }

    fn cmd(&self) -> impl IntoIterator<Item = impl Into<Cow<'_, str>>> {
        vec!["-c", "fsync=off"]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Postgres,
}

impl Backend {
    pub fn all() -> [Backend; 2] {
        [Backend::Sqlite, Backend::Postgres]
    }

    fn from_env() -> Backend {
        match std::env::var("RUSTCHAT_E2E_BACKEND").as_deref() {
            Ok("postgres") => Backend::Postgres,
            _ => Backend::Sqlite,
        }
    }

    fn env_value(self) -> &'static str {
        match self {
            Backend::Sqlite => "sqlite",
            Backend::Postgres => "postgres",
        }
    }
}

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rustchat-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct ServerGuard {
    child: Child,
    log: PathBuf,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn container_runtime() -> Option<&'static str> {
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        if !host.trim().is_empty() {
            return Some("env");
        }
    }
    const SOCKETS: [&str; 3] = [
        "/var/run/docker.sock",
        "/run/user/1000/podman/podman.sock",
        "/run/podman/podman.sock",
    ];
    SOCKETS
        .iter()
        .find(|path| Path::new(path).exists())
        .copied()
}

pub fn postgres_available() -> bool {
    container_runtime().is_some()
}

fn configure_container_env() {
    if std::env::var("DOCKER_HOST").is_ok() {
        return;
    }
    for path in [
        "/run/user/1000/podman/podman.sock",
        "/run/podman/podman.sock",
    ] {
        if Path::new(path).exists() {
            std::env::set_var("DOCKER_HOST", format!("unix://{path}"));
            break;
        }
    }
    std::env::set_var("TESTCONTAINERS_RYUK_DISABLED", "true");
}

pub struct TestEnv {
    pub base_url: String,
    pub provider_url: String,
    pub sandbox_url: String,
    pub provider: FakeProvider,
    pub sandbox: FakeSandbox,
    pub http: reqwest::Client,
    pub data_dir: TempDir,
    _server: ServerGuard,
    _pg: Option<ContainerAsync<PgVector>>,
}

impl TestEnv {
    pub fn server_pid(&self) -> u32 {
        self._server.child.id()
    }
}

#[derive(Clone)]
pub struct TestEnvBuilder {
    backend: Backend,
    script: Script,
    sandbox_tool: bool,
    extra_env: Vec<(String, String)>,
}

impl TestEnvBuilder {
    pub fn new() -> Self {
        Self {
            backend: Backend::from_env(),
            script: Script::text("Hello from the fake provider"),
            sandbox_tool: false,
            extra_env: Vec::new(),
        }
    }

    pub fn backend(mut self, backend: Backend) -> Self {
        self.backend = backend;
        self
    }

    pub fn script(mut self, script: Script) -> Self {
        self.script = script;
        self
    }

    pub fn sandbox_tool(mut self, enabled: bool) -> Self {
        self.sandbox_tool = enabled;
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    pub async fn start(self) -> Result<TestEnv> {
        let data_dir = TempDir::new();
        let (sandbox, sandbox_url) = FakeSandbox::start().await;
        let (provider, provider_url) = FakeProvider::start(self.script).await;

        let (database_url, pg) = match self.backend {
            Backend::Sqlite => {
                let db = data_dir.path().join("chat.db");
                (format!("sqlite://{}", db.display()), None)
            }
            Backend::Postgres => {
                if !postgres_available() {
                    return Err(anyhow!(
                        "RUSTCHAT_E2E_BACKEND=postgres was requested but no container runtime is                          reachable (Docker/Podman socket not found)"
                    ));
                }
                configure_container_env();
                let container = PgVector
                    .start()
                    .await
                    .map_err(|err| anyhow!("starting postgres container: {err}"))?;
                let host = container.get_host().await?.to_string();
                let port = container.get_host_port_ipv4(5432).await?;
                let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
                (url, Some(container))
            }
        };

        let port = free_port()?;
        let bind_addr = format!("127.0.0.1:{port}");
        let base_url = format!("http://{bind_addr}");

        let log = data_dir.path().join("server.log");
        let mut command = Command::new(env!("CARGO_BIN_EXE_chat-server"));
        command
            .current_dir(data_dir.path())
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", data_dir.path())
            .env("DATABASE_URL", &database_url)
            .env("DB_BACKEND", self.backend.env_value())
            .env("BIND_ADDR", &bind_addr)
            .env("JWT_SECRET", "e2e-test-secret-that-is-long-enough-1234")
            .env("FILE_STORAGE_DIR", data_dir.path().join("files"))
            .env("RATE_LIMIT_ENABLED", "false")
            .env("SANDBOXD_URL", &sandbox_url)
            .env(
                "SANDBOX_TOOL_ENABLED",
                if self.sandbox_tool { "true" } else { "false" },
            )
            .env("RUST_LOG", "warn")
            .env("OPENAI_API_KEY", "fake-provider-key");
        for (key, value) in &self.extra_env {
            command.env(key, value);
        }

        let log_file = std::fs::File::create(&log).unwrap();
        let child = command
            .stdout(Stdio::null())
            .stderr(Stdio::from(log_file))
            .spawn()
            .map_err(|err| anyhow!("spawning chat-server: {err}"))?;

        let mut server = ServerGuard { child, log };
        wait_health(&base_url, &mut server).await?;

        Ok(TestEnv {
            base_url,
            provider_url,
            sandbox_url,
            provider,
            sandbox,
            http: reqwest::Client::new(),
            data_dir,
            _server: server,
            _pg: pg,
        })
    }
}

impl Default for TestEnvBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

async fn wait_health(base_url: &str, server: &mut ServerGuard) -> Result<()> {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(Some(status)) = server.child.try_wait() {
            let log = std::fs::read_to_string(&server.log).unwrap_or_default();
            return Err(anyhow!("chat-server exited with {status}:\n{log}"));
        }
        if let Ok(resp) = client.get(format!("{base_url}/health")).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let log = std::fs::read_to_string(&server.log).unwrap_or_default();
            return Err(anyhow!("chat-server did not become healthy:\n{log}"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
