//! In-process registry of live assistant generations.
//!
//! Each streamed assistant message publishes its full accumulated content over
//! a [`tokio::sync::watch`] channel. Streaming full snapshots (rather than
//! deltas) means a subscriber that joins late or reconnects can reconstruct the
//! reply from a single value with no gaps.
//!
//! This registry is per-process: a client that reconnects to a *different*
//! replica cannot rejoin a live generation and falls back to the persisted
//! message (see `routes::resume_stream`). Cross-replica resume needs a shared
//! bus; tracked as a follow-up in `PLAN.md`.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::watch;
use uuid::Uuid;

/// Latest state of a generation, broadcast to subscribers.
#[derive(Debug, Clone)]
pub enum StreamState {
    /// Still generating; `content` is everything accumulated so far.
    Streaming { content: String },
    /// Finished successfully.
    Completed { content: String },
    /// Failed after producing `content`; `error` is a human-readable message.
    Failed { content: String, error: String },
}

#[derive(Default)]
pub struct StreamHub {
    senders: Mutex<HashMap<Uuid, watch::Sender<StreamState>>>,
}

impl StreamHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new generation and return its sender. Producers publish every
    /// checkpoint and a single terminal state.
    pub fn open(&self, message_id: Uuid) -> watch::Sender<StreamState> {
        let (tx, _rx) = watch::channel(StreamState::Streaming {
            content: String::new(),
        });
        self.senders.lock().unwrap().insert(message_id, tx.clone());
        tx
    }

    /// Subscribe to a live generation, if one is running.
    pub fn subscribe(&self, message_id: Uuid) -> Option<watch::Receiver<StreamState>> {
        self.senders
            .lock()
            .unwrap()
            .get(&message_id)
            .map(watch::Sender::subscribe)
    }

    /// Remove a generation once it has reached a terminal state.
    pub fn close(&self, message_id: Uuid) {
        self.senders.lock().unwrap().remove(&message_id);
    }
}
