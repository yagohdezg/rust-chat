//! The computer control plane: placement, lifecycle and idle reaping.
//!
//! [`ComputerOrchestrator`] ties the placement registry ([`Store`]) to the
//! sandbox [`NodeRouter`]. Given a user it provisions (or resumes) their
//! persistent computer, routes executions to the node that hosts it, and
//! destroys computers that have gone idle. A small in-memory warm pool hides
//! box cold-start on the hot path.
//!
//! When workspace volumes are enabled each computer mounts a durable volume
//! (named from its user) at the working directory, so files survive the box
//! being stopped, snapshotted, reaped or re-provisioned on another node.

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
    ready: std::collections::HashMap<String, Vec<String>>,
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
    /// Mount a durable per-user workspace volume at the box's working directory.
    workspace: bool,
    warm: Mutex<WarmPool>,
}

impl ComputerOrchestrator {
    pub fn new(
        store: Arc<dyn Store>,
        router: NodeRouter,
        idle_ttl: Duration,
        warm_pool_size: usize,
        workspace: bool,
    ) -> Self {
        Self {
            store,
            router,
            idle_ttl: ChronoDuration::from_std(idle_ttl)
                .unwrap_or_else(|_| ChronoDuration::seconds(i64::MAX)),
            warm_pool_size,
            workspace,
            warm: Mutex::new(WarmPool::default()),
        }
    }

    /// Durable workspace volume name for a user, or `None` when disabled.
    ///
    /// Deterministic from the user id, so a reaped computer re-provisioned on
    /// any node remounts the same volume and finds its files again.
    fn workspace_for(&self, user_id: Uuid) -> Option<String> {
        self.workspace
            .then(|| format!("rustchat-{}", user_id.simple()))
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
                self.activate(&computer).await?;
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

        let workspace = self.workspace_for(user_id);
        let handle = match workspace {
            // A volume-mounted box is per-user, so a blank warm box cannot be
            // reused; always create fresh.
            Some(ref name) => node.create(Some(name)).await?,
            None => match self.warm.lock().await.take(&node_name) {
                Some(handle) => handle,
                None => node.create(None).await?,
            },
        };

        match self
            .store
            .create_computer(user_id, &node_name, Some(&handle), workspace.as_deref())
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

    /// Give an existing computer a fresh box on its node, remounting the
    /// workspace volume. Used when the box behind a placement is gone.
    async fn reattach(&self, computer: &Computer) -> Result<()> {
        let node = self
            .router
            .get(&computer.node)
            .ok_or_else(|| ComputerError::UnknownNode(computer.node.clone()))?;
        let handle = node.create(computer.volume.as_deref()).await?;
        self.store
            .set_computer_handle(computer.id, Some(&handle))
            .await?;
        Ok(())
    }

    /// Bring a computer's box back to `running`, re-provisioning if it is gone.
    async fn activate(&self, computer: &Computer) -> Result<()> {
        if let (Some(node), Some(handle)) =
            (self.router.get(&computer.node), computer.handle.as_deref())
        {
            if node.resume(handle).await.is_ok() {
                return Ok(());
            }
            tracing::warn!(
                computer = %computer.id,
                node = %computer.node,
                "computer box did not resume; re-provisioning from workspace"
            );
        }
        self.reattach(computer).await
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

        match node.exec(&handle, request).await {
            Ok(result) => {
                self.store.touch_computer(computer.id).await?;
                Ok(result)
            }
            Err(err) => {
                // The box may have been lost (node restart, eviction). Free the
                // stale placement, re-provision from the workspace volume, and
                // retry once so a restart is invisible to the caller.
                tracing::warn!(
                    error = %err,
                    computer = %computer.id,
                    "computer exec failed; re-provisioning and retrying"
                );
                self.release(&computer).await;
                self.reattach(&computer).await?;
                let computer = self
                    .store
                    .get_computer(computer.id)
                    .await?
                    .ok_or(ComputerError::MissingHandle)?;
                let handle = computer
                    .handle
                    .clone()
                    .ok_or(ComputerError::MissingHandle)?;
                let result = node.exec(&handle, request).await?;
                self.store.touch_computer(computer.id).await?;
                Ok(result)
            }
        }
    }

    /// Pause the user's computer, keeping its placement. Idempotent.
    pub async fn pause(&self, user_id: Uuid) -> Result<Option<Computer>> {
        let Some(computer) = self.store.get_live_computer_for_user(user_id).await? else {
            return Ok(None);
        };
        if computer.state != computer_state::PAUSED {
            if let (Some(node), Some(handle)) =
                (self.router.get(&computer.node), computer.handle.as_deref())
            {
                if let Err(e) = node.pause(handle).await {
                    tracing::warn!(error = %e, computer = %computer.id, "failed to pause computer box");
                }
            }
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
        self.activate(&computer).await?;
        if computer.state != computer_state::RUNNING {
            self.store
                .set_computer_state(computer.id, computer_state::RUNNING)
                .await?;
        }
        self.store.touch_computer(computer.id).await?;
        Ok(self.store.get_computer(computer.id).await?)
    }

    /// Destroy the user's computer and free its box. Returns whether one
    /// existed. Deletes the workspace volume: an explicit destroy is final.
    pub async fn destroy(&self, user_id: Uuid) -> Result<bool> {
        let Some(computer) = self.store.get_live_computer_for_user(user_id).await? else {
            return Ok(false);
        };
        self.release(&computer).await;
        if let Some(volume) = computer.volume.as_deref() {
            self.remove_workspace(&computer.node, volume).await;
        }
        self.store.set_computer_handle(computer.id, None).await?;
        self.store
            .set_computer_state(computer.id, computer_state::DESTROYED)
            .await?;
        Ok(true)
    }

    /// Destroy computers idle for longer than the TTL. Returns how many.
    ///
    /// The workspace volume is retained, so a user returning after the idle
    /// window finds their files when the computer is re-provisioned.
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

    /// Best-effort workspace volume teardown on a node.
    async fn remove_workspace(&self, node_name: &str, volume: &str) {
        let Some(node) = self.router.get(node_name) else {
            return;
        };
        if let Err(e) = node.destroy_workspace(volume).await {
            tracing::warn!(
                error = %e,
                volume,
                node = %node_name,
                "failed to destroy computer workspace volume"
            );
        }
    }

    /// Top up the warm pool to `warm_pool_size` boxes per node.
    ///
    /// Disabled when workspace volumes are enabled: warm boxes are blank and
    /// have no volume, so they cannot host a per-user workspace.
    pub async fn refill_warm_pool(&self) -> Result<usize> {
        if self.warm_pool_size == 0 || self.workspace {
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
                match node.create(None).await {
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
