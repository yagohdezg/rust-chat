//! Per-user persistent "computer" control plane.
//!
//! A *computer* is a long-lived, per-user execution workspace (a BoxLite
//! microVM today) that outlives any single chat session. This crate owns the
//! control plane around it:
//!
//! * [`NodeRouter`] / [`ComputerNode`] — resolving a placement to a callable
//!   sandbox node (no sticky load balancer).
//! * [`ComputerOrchestrator`] — create / pause / resume / destroy, idle-TTL
//!   reaping, and a small warm pool.
//!
//! Placement is persisted through `chat_store::Store`, so routing survives
//! process restarts. The node protocol is spoken by `sandboxd`
//! (`/v1/computers*`), which drives BoxLite persistent boxes.

mod error;
mod node;
mod orchestrator;

pub use error::{ComputerError, Result};
pub use node::{ComputerNode, HttpComputerNode, NodeRouter};
pub use orchestrator::ComputerOrchestrator;

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use uuid::Uuid;

    use chat_sandbox::{ExecRequest, ExecResult};
    use chat_store::types::computer_state;
    use chat_store::Store;

    use super::*;

    /// In-memory [`ComputerNode`] that records the calls it receives.
    struct MockNode {
        name: String,
        counter: StdMutex<u64>,
        created: StdMutex<u64>,
        destroyed: StdMutex<Vec<String>>,
        execs: StdMutex<u64>,
    }

    impl MockNode {
        fn new(name: &str) -> Arc<Self> {
            Arc::new(Self {
                name: name.into(),
                counter: StdMutex::new(0),
                created: StdMutex::new(0),
                destroyed: StdMutex::new(Vec::new()),
                execs: StdMutex::new(0),
            })
        }

        fn created(&self) -> u64 {
            *self.created.lock().unwrap()
        }

        fn destroyed(&self) -> Vec<String> {
            self.destroyed.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ComputerNode for MockNode {
        fn name(&self) -> &str {
            &self.name
        }

        async fn create(&self) -> Result<String> {
            let mut counter = self.counter.lock().unwrap();
            *counter += 1;
            *self.created.lock().unwrap() += 1;
            Ok(format!("{}-box-{}", self.name, *counter))
        }

        async fn exec(&self, _handle: &str, _request: &ExecRequest) -> Result<ExecResult> {
            *self.execs.lock().unwrap() += 1;
            Ok(ExecResult {
                exit_code: 0,
                stdout: "ok".into(),
                stderr: String::new(),
                timed_out: false,
                truncated: false,
                files: Vec::new(),
            })
        }

        async fn destroy(&self, handle: &str) -> Result<()> {
            self.destroyed.lock().unwrap().push(handle.to_string());
            Ok(())
        }

        async fn health(&self) -> Result<()> {
            Ok(())
        }
    }

    async fn test_store() -> (Arc<dyn Store>, Uuid) {
        let path = std::env::temp_dir().join(format!("rustchat-computers-{}.db", Uuid::new_v4()));
        let url = format!("sqlite://{}", path.display());
        let secrets = Arc::new(chat_core::SecretCipher::derive_from_secret("test").unwrap());
        let db = chat_db_sqlite::SqliteStore::connect(&url, secrets)
            .await
            .unwrap();
        db.migrate().await.unwrap();
        let user = db
            .create_user("u@example.com", None, "hash", "user")
            .await
            .unwrap();
        (Arc::new(db), user.id)
    }

    fn orchestrator(
        store: Arc<dyn Store>,
        node: Arc<dyn ComputerNode>,
        idle_ttl: Duration,
        warm_pool_size: usize,
    ) -> ComputerOrchestrator {
        ComputerOrchestrator::new(
            store,
            NodeRouter::new().with_node(node),
            idle_ttl,
            warm_pool_size,
        )
    }

    #[tokio::test]
    async fn ensure_running_is_idempotent() {
        let (store, user) = test_store().await;
        let node = MockNode::new("node-a");
        let orch = orchestrator(store, node.clone(), Duration::from_secs(1800), 0);

        let first = orch.ensure_running(user).await.unwrap();
        let second = orch.ensure_running(user).await.unwrap();

        assert_eq!(first.id, second.id, "one computer per user");
        assert_eq!(first.state, computer_state::RUNNING);
        assert_eq!(node.created(), 1, "box provisioned once");
    }

    #[tokio::test]
    async fn pause_and_resume_keep_placement() {
        let (store, user) = test_store().await;
        let node = MockNode::new("node-a");
        let orch = orchestrator(store, node.clone(), Duration::from_secs(1800), 0);

        let computer = orch.ensure_running(user).await.unwrap();
        let paused = orch.pause(user).await.unwrap().unwrap();
        assert_eq!(paused.id, computer.id);
        assert_eq!(paused.state, computer_state::PAUSED);

        // Resuming the same row does not provision a second box.
        let resumed = orch.resume(user).await.unwrap().unwrap();
        assert_eq!(resumed.id, computer.id);
        assert_eq!(resumed.state, computer_state::RUNNING);
        assert_eq!(node.created(), 1);
    }

    #[tokio::test]
    async fn destroy_releases_box_and_recreates() {
        let (store, user) = test_store().await;
        let node = MockNode::new("node-a");
        let orch = orchestrator(store, node.clone(), Duration::from_secs(1800), 0);

        let first = orch.ensure_running(user).await.unwrap();
        assert!(orch.destroy(user).await.unwrap());
        assert_eq!(node.destroyed(), vec![first.handle.clone().unwrap()]);
        assert!(orch.current(user).await.unwrap().is_none());

        let second = orch.ensure_running(user).await.unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(node.created(), 2);
    }

    #[tokio::test]
    async fn exec_routes_to_the_hosting_node() {
        let (store, user) = test_store().await;
        let node = MockNode::new("node-a");
        let orch = orchestrator(store, node.clone(), Duration::from_secs(1800), 0);

        let result = orch
            .exec(
                user,
                &ExecRequest {
                    language: "python".into(),
                    code: "print(1)".into(),
                    files: Vec::new(),
                    outputs: Vec::new(),
                },
            )
            .await
            .unwrap();

        assert_eq!(result.stdout, "ok");
        assert_eq!(*node.execs.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn idle_computers_are_reaped() {
        let (store, user) = test_store().await;
        let node = MockNode::new("node-a");
        let orch = orchestrator(store, node.clone(), Duration::from_millis(0), 0);

        let computer = orch.ensure_running(user).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert_eq!(orch.reap_idle().await.unwrap(), 1);
        assert_eq!(node.destroyed(), vec![computer.handle.unwrap()]);
        assert!(orch.current(user).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn warm_pool_is_refilled_and_consumed() {
        let (store, user) = test_store().await;
        let node = MockNode::new("node-a");
        let orch = orchestrator(store, node.clone(), Duration::from_secs(1800), 1);

        assert_eq!(orch.refill_warm_pool().await.unwrap(), 1);
        // Idempotent: already at target.
        assert_eq!(orch.refill_warm_pool().await.unwrap(), 0);

        let computer = orch.ensure_running(user).await.unwrap();
        // The warm box was reused, so `create` ran once (during refill) only.
        assert_eq!(node.created(), 1);
        assert_eq!(computer.handle.as_deref(), Some("node-a-box-1"));
    }

    #[tokio::test]
    async fn placement_spreads_across_nodes() {
        let (store, user_a) = test_store().await;
        let user_b = store
            .create_user("b@example.com", None, "hash", "user")
            .await
            .unwrap()
            .id;
        let node_a = MockNode::new("node-a");
        let node_b = MockNode::new("node-b");
        let orch = ComputerOrchestrator::new(
            store,
            NodeRouter::new()
                .with_node(node_a.clone())
                .with_node(node_b.clone()),
            Duration::from_secs(1800),
            0,
        );

        let first = orch.ensure_running(user_a).await.unwrap();
        let second = orch.ensure_running(user_b).await.unwrap();

        // Ties break by (sorted) node name, then the second user goes to the
        // emptier node.
        assert_eq!(first.node, "node-a");
        assert_eq!(second.node, "node-b");
    }

    #[tokio::test]
    async fn http_node_round_trips_over_the_wire() {
        use axum::extract::Path;
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::{delete, post};
        use axum::{Json, Router};

        async fn create(headers: HeaderMap) -> (StatusCode, Json<serde_json::Value>) {
            assert_eq!(
                headers.get("authorization").and_then(|v| v.to_str().ok()),
                Some("Bearer node-token")
            );
            (
                StatusCode::CREATED,
                Json(serde_json::json!({ "handle": "box-9" })),
            )
        }

        async fn exec(
            Path(handle): Path<String>,
            Json(_request): Json<ExecRequest>,
        ) -> Json<ExecResult> {
            assert_eq!(handle, "box-9");
            Json(ExecResult {
                exit_code: 0,
                stdout: "hi".into(),
                stderr: String::new(),
                timed_out: false,
                truncated: false,
                files: Vec::new(),
            })
        }

        async fn destroy(Path(handle): Path<String>) -> StatusCode {
            assert_eq!(handle, "box-9");
            StatusCode::NO_CONTENT
        }

        let app = Router::new()
            .route("/v1/computers", post(create))
            .route("/v1/computers/{handle}/exec", post(exec))
            .route("/v1/computers/{handle}", delete(destroy));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let node = HttpComputerNode::new(
            "node-a",
            format!("http://{addr}"),
            Some("node-token".into()),
        );
        let handle = node.create().await.unwrap();
        assert_eq!(handle, "box-9");

        let result = node
            .exec(
                &handle,
                &ExecRequest {
                    language: "python".into(),
                    code: "print('hi')".into(),
                    files: Vec::new(),
                    outputs: Vec::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(result.stdout, "hi");

        node.destroy(&handle).await.unwrap();
    }
}
