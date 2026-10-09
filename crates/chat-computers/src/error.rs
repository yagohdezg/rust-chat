use chat_core::ChatError;
use chat_sandbox::SandboxError;

/// Errors from the computer control plane (placement + node calls).
#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    /// No sandbox nodes were configured, so nothing can be placed.
    #[error("no sandbox nodes are configured")]
    NoNodes,
    /// The registry points at a node the router does not know about.
    #[error("unknown sandbox node `{0}`")]
    UnknownNode(String),
    /// A computer row exists but has no box handle to run against.
    #[error("computer has no active handle")]
    MissingHandle,
    /// The node (sandboxd) refused or failed the operation.
    #[error(transparent)]
    Node(#[from] SandboxError),
    /// The placement registry (relational store) failed.
    #[error(transparent)]
    Store(#[from] ChatError),
    /// Any other control-plane failure.
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ComputerError>;
