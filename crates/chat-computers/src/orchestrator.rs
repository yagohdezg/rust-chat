//! The computer control plane: placement, lifecycle and idle reaping.
//!
//! [`ComputerOrchestrator`] ties the placement registry ([`Store`]) to the
//! sandbox [`NodeRouter`]. Given a user it provisions (or resumes) their
//! persistent computer, routes executions to the node that hosts it, and
//! destroys computers that have gone idle. A small in-memory warm pool hides
//! box cold-start on the hot path.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use tokio::sync::Mutex;
use uuid::Uuid;

use chat_sandbox::{ExecRequest, ExecResult};
use chat_store::types::computer_state;
use chat_store::{Computer, Store};

use crate::error::{ComputerError, Result};
use crate::node::NodeRouter;

/// Idle computers destroyed per reaper pass, bounding node calls per tick.
const REAP_BATCH: i64 = 100;

/// Pre-provisioned blank box handles, keyed by node name.
#[derive(Default)]
struct WarmPool {
    ready: HashMap<String, Vec<String>>,
}

impl WarmPool {
    fn take(&mut self, node: &str) -> Option<String> {
        self.ready.get_mut(node).and_then(Vec::pop)
    }

    fn put(&mut self, node: &str, handle: String) {
        self.ready.entry(node.to_string()).or_default().push(handle);
    }

    fn len(&self, node: &str) -> usize {
        self.ready.get(node).map_or(0, Vec::len)
    }
}

/// Per-user persistent "computer" control plane.
pub struct ComputerOrchestrator {
    store: Arc<dyn Store>,
    router: NodeRouter,
    /// A `running`/`paused` computer untouched for this long is destroyed.
    idle_ttl: ChronoDuration,
    /// Target number of pre-provisioned boxes per node (`0` disables).
    warm_pool_size: usize,
    warm: Mutex<WarmPool>,
}

impl ComputerOrchestrator {
    pub fn new(
        store: Arc<dyn Store>,
        router: NodeRouter,
        idle_ttl: Duration,
        warm_pool_size: usize,
    ) -> Self {
        Self {
            store,
            router,
            idle_ttl: ChronoDuration::from_std(idle_ttl)
                .unwrap_or_else(|_| ChronoDuration::seconds(i64::MAX)),
            warm_pool_size,
            warm: Mutex::new(WarmPool::default()),
        }
    }

    /// The user's current computer, without provisioning one.
    pub async fn current(&self, user_id: Uuid) -> Result<Option<Computer>> {
        Ok(self.store.get_live_computer_for_user(user_id).await?)
    }

    /// Return the user's live computer, creating or resuming it as needed.
    ///
    /// Idempotent: concurrent callers converge on a single computer for a user
    /// because the registry enforces one live row per user.
    pub async fn ensure_running(&self, user_id: Uuid) -> Result<Computer> {
        if let Some(computer) = self.store.get_live_computer_for_user(user_id).await? {
            if computer.state != computer_state::RUNNING {
                self.store
                    .set_computer_state(computer.id, computer_state::RUNNING)
                    .await?;
            }
            self.store.touch_computer(computer.id).await?;
            return self
                .store
                .get_computer(computer.id)
                .await?
                .ok_or(ComputerError::MissingHandle);
        }
        self.provision(user_id).await
    }

    /// Provision a fresh computer for a user on the least-loaded node.
    async fn provision(&self, user_id: Uuid) -> Result<Computer> {
        let node_name = self.pick_node().await?;
        let node = self
            .router
            .get(&node_name)
            .ok_or_else(|| ComputerError::UnknownNode(node_name.clone()))?
            .clone();

        let handle = match self.warm.lock().await.take(&node_name) {
            Some(handle) => handle,
            None => node.create().await?,
        };

        match self
            .store
            .create_computer(user_id, &node_name, Some(&handle))
            .await
        {
            Ok(computer) => Ok(computer),
            Err(err) => {
                // Lost a placement race (or DB error): don't leak the box, and
                // prefer the winner's row if one now exists.
                if let Err(e) = node.destroy(&handle).await {
                    tracing::warn!(error = %e, "failed to roll back provisioned box");
                }
                if let Some(existing) = self.store.get_live_computer_for_user(user_id).await? {
                    return Ok(existing);
                }
                Err(err.into())
            }
        }
    }

    /// Choose the node with the fewest live computers (ties break by name).
    async fn pick_node(&self) -> Result<String> {
        let mut best: Option<(String, i64)> = None;
        for name in self.router.names() {
            let count = self.store.count_live_computers_on_node(&name).await?;
            match &best {
                Some((_, best_count)) if *best_count <= count => {}
                _ => best = Some((name, count)),
            }
        }
        best.map(|(name, _)| name).ok_or(ComputerError::NoNodes)
    }

    /// Run a request on the user's computer, provisioning it if needed.
    pub async fn exec(&self, user_id: Uuid, request: &ExecRequest) -> Result<ExecResult> {
        let computer = self.ensure_running(user_id).await?;
        let handle = computer
            .handle
            .clone()
            .ok_or(ComputerError::MissingHandle)?;
        let node = self
            .router
            .get(&computer.node)
            .ok_or_else(|| ComputerError::UnknownNode(computer.node.clone()))?;
        let result = node.exec(&handle, request).await?;
        self.store.touch_computer(computer.id).await?;
        Ok(result)
    }

    /// Pause the user's computer, keeping its placement. Idempotent.
    pub async fn pause(&self, user_id: Uuid) -> Result<Option<Computer>> {
        let Some(computer) = self.store.get_live_computer_for_user(user_id).await? else {
            return Ok(None);
        };
        if computer.state != computer_state::PAUSED {
            self.store
                .set_computer_state(computer.id, computer_state::PAUSED)
                .await?;
        }
        Ok(self.store.get_computer(computer.id).await?)
    }

    /// Resume a paused computer. Idempotent.
    pub async fn resume(&self, user_id: Uuid) -> Result<Option<Computer>> {
        let Some(computer) = self.store.get_live_computer_for_user(user_id).await? else {
            return Ok(None);
        };
        if computer.state != computer_state::RUNNING {
            self.store
                .set_computer_state(computer.id, computer_state::RUNNING)
                .await?;
        }
        self.store.touch_computer(computer.id).await?;
        Ok(self.store.get_computer(computer.id).await?)
    }

    /// Destroy the user's computer and free its box. Returns whether one
    /// existed.
    pub async fn destroy(&self, user_id: Uuid) -> Result<bool> {
        let Some(computer) = self.store.get_live_computer_for_user(user_id).await? else {
            return Ok(false);
        };
        self.release(&computer).await;
        self.store.set_computer_handle(computer.id, None).await?;
        self.store
            .set_computer_state(computer.id, computer_state::DESTROYED)
            .await?;
        Ok(true)
    }

    /// Destroy computers idle for longer than the TTL. Returns how many.
    pub async fn reap_idle(&self) -> Result<usize> {
        let cutoff = Utc::now() - self.idle_ttl;
        let idle = self.store.list_idle_computers(cutoff, REAP_BATCH).await?;
        let mut reaped = 0;
        for computer in idle {
            self.release(&computer).await;
            self.store.set_computer_handle(computer.id, None).await?;
            self.store
                .set_computer_state(computer.id, computer_state::DESTROYED)
                .await?;
            reaped += 1;
        }
        Ok(reaped)
    }

    /// Best-effort box teardown on a node; a stale registry row is still
    /// repaired by the caller even when the node call fails.
    async fn release(&self, computer: &Computer) {
        let (Some(node), Some(handle)) =
            (self.router.get(&computer.node), computer.handle.as_deref())
        else {
            return;
        };
        if let Err(e) = node.destroy(handle).await {
            tracing::warn!(
                error = %e,
                computer = %computer.id,
                node = %computer.node,
                "failed to destroy computer box on node"
            );
        }
    }

    /// Top up the warm pool to `warm_pool_size` boxes per node.
    pub async fn refill_warm_pool(&self) -> Result<usize> {
        if self.warm_pool_size == 0 {
            return Ok(0);
        }
        let mut created = 0;
        for name in self.router.names() {
            let node = match self.router.get(&name) {
                Some(node) => node.clone(),
                None => continue,
            };
            let have = self.warm.lock().await.len(&name);
            for _ in have..self.warm_pool_size {
                match node.create().await {
                    Ok(handle) => {
                        self.warm.lock().await.put(&name, handle);
                        created += 1;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, node = %name, "warm pool refill failed");
                        break;
                    }
                }
            }
        }
        Ok(created)
    }

    /// Probe every node's health, for readiness reporting.
    pub async fn health(&self) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        for name in self.router.names() {
            let healthy = match self.router.get(&name) {
                Some(node) => node.health().await.is_ok(),
                None => false,
            };
            out.push((name, healthy));
        }
        out
    }
}
