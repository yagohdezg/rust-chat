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
use chat_sandbox::{ExecRequest, ExecResult, SandboxBackend};

use crate::workspace::FileWorkspace;

/// The execution backend behind `execute_code`.
///
/// The persistent per-user computer is preferred when enabled; otherwise the
/// shared, ephemeral `sandboxd` backend is used.
pub enum CodeBackend {
    Ephemeral(Arc<dyn SandboxBackend>),
    Computer(Arc<ComputerOrchestrator>),
}

/// Binds the agent runtime's [`UserSandbox`] port to one conversation.
///
/// Every call loads the conversation's attached files into the sandbox and
/// saves anything the run produced in `/app/output` back into the user's
/// library. The backend guarantees the same user always lands in the same
/// persistent workspace (or a fresh ephemeral box), so file state behaves as
/// the user expects.
struct ConversationSandbox {
    backend: CodeBackend,
    workspace: Arc<FileWorkspace>,
    conversation_id: Uuid,
}

#[async_trait]
impl UserSandbox for ConversationSandbox {
    async fn exec_for_user(
        &self,
        user_id: Uuid,
        request: &ExecRequest,
    ) -> anyhow::Result<ExecResult> {
        let mut request = request.clone();
        request.files = self
            .workspace
            .load_for_conversation(user_id, self.conversation_id)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let result = match &self.backend {
            CodeBackend::Ephemeral(sandbox) => sandbox.run(&request).await?,
            CodeBackend::Computer(orchestrator) => orchestrator.exec(user_id, &request).await?,
        };

        if let Err(err) = self
            .workspace
            .persist_outputs(user_id, self.conversation_id, &result.files)
            .await
        {
            tracing::warn!(error = %err, "failed to persist sandbox output files");
        }
        Ok(result)
    }
}

/// Build the `execute_code` tool for `user_id`, scoped to `conversation_id`, so
/// code can read the conversation's files and save outputs back to the library.
pub fn conversation_code_tool(
    backend: CodeBackend,
    workspace: Arc<FileWorkspace>,
    user_id: Uuid,
    conversation_id: Uuid,
) -> Arc<dyn Tool> {
    Arc::new(UserCodeInterpreterTool::new(
        Arc::new(ConversationSandbox {
            backend,
            workspace,
            conversation_id,
        }),
        user_id,
    ))
}
