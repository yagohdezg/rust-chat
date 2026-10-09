//! Tools that depend on per-request context.
//!
//! The base [`chat_agents::ToolRegistry`] in [`crate::state::AppState`] is built
//! once at boot and shared across every user, so any tool that must act *as a
//! user* cannot live there. Instead the chat path constructs such tools per
//! turn — see `resolve_agent` in [`crate::routes`].

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use chat_agents::{Tool, UserCodeInterpreterTool, UserSandbox};
use chat_computers::ComputerOrchestrator;
use chat_sandbox::{ExecRequest, ExecResult};

/// Adapts the per-user computer control plane to the agent runtime's
/// [`UserSandbox`] port.
///
/// Routing through [`ComputerOrchestrator::exec`] is what makes the sandbox
/// *unique per user*: the placement registry enforces a single live computer
/// per user, so every call from a user lands in the same persistent box.
struct ComputerSandbox {
    orchestrator: Arc<ComputerOrchestrator>,
}

#[async_trait]
impl UserSandbox for ComputerSandbox {
    async fn exec_for_user(
        &self,
        user_id: Uuid,
        request: &ExecRequest,
    ) -> anyhow::Result<ExecResult> {
        Ok(self.orchestrator.exec(user_id, request).await?)
    }
}

/// Build the `execute_code` tool backed by `user_id`'s persistent computer.
pub fn computer_code_interpreter(
    orchestrator: Arc<ComputerOrchestrator>,
    user_id: Uuid,
) -> Arc<dyn Tool> {
    Arc::new(UserCodeInterpreterTool::new(
        Arc::new(ComputerSandbox { orchestrator }),
        user_id,
    ))
}
