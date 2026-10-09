/// Lifecycle values for [`crate::Message::status`].
///
/// Only assistant replies stream; other roles are always `complete`.
pub mod message_status {
    /// Generation finished successfully.
    pub const COMPLETE: &str = "complete";
    /// The model is still streaming into this message.
    pub const STREAMING: &str = "streaming";
    /// Generation failed with an error after partial output.
    pub const ERROR: &str = "error";
    /// The server stopped before generation finished (e.g. restart).
    pub const INTERRUPTED: &str = "interrupted";
}

/// Lifecycle values for [`crate::Computer::state`].
///
/// A computer starts out `provisioning` while the box is being placed, then
/// becomes `running`. `paused` keeps the placement but stops routing new work
/// until it is resumed; `destroyed` is terminal (the box is gone and the slot
/// is free).
pub mod computer_state {
    /// The box is being created on its node.
    pub const PROVISIONING: &str = "provisioning";
    /// The computer is live and serving executions.
    pub const RUNNING: &str = "running";
    /// The computer is idle but its placement is retained.
    pub const PAUSED: &str = "paused";
    /// Terminal: the box has been removed. The row is kept for audit.
    pub const DESTROYED: &str = "destroyed";
}
