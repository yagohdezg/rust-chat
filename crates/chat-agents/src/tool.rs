use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use uuid::Uuid;

use chat_sandbox::{ExecRequest, ExecResult};

/// A callable tool exposed to the model.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON Schema for the tool arguments.
    fn schema(&self) -> Value;
    async fn call(&self, arguments: Value) -> anyhow::Result<String>;
}

/// Runs sandboxed code on behalf of a specific user.
///
/// The shared [`ToolRegistry`] has no notion of who is calling, so any tool
/// that must act *as a user* is built per request. The server implements this
/// with its per-user "computer" control plane, which guarantees the same user
/// always lands in the same persistent workspace.
#[async_trait::async_trait]
pub trait UserSandbox: Send + Sync {
    async fn exec_for_user(
        &self,
        user_id: Uuid,
        request: &ExecRequest,
    ) -> anyhow::Result<ExecResult>;
}

/// Registry of tools addressable by name, with OpenAI tool definitions.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// A new registry containing only the named tools that are registered,
    /// preserving nothing else. Used to give an agent the exact tools its
    /// config enables.
    pub fn restricted(&self, names: &[String]) -> ToolRegistry {
        let mut subset = ToolRegistry::new();
        for name in names {
            if let Some(tool) = self.tools.get(name) {
                subset.tools.insert(name.clone(), tool.clone());
            }
        }
        subset
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// OpenAI `tools[]` definitions for every registered tool.
    pub fn definitions(&self) -> Vec<Value> {
        self.tools
            .values()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.schema(),
                    }
                })
            })
            .collect()
    }
}
