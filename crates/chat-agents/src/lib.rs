//! Agent runtime: provider streaming + a tool-call execution loop.

pub mod builtin;
pub mod runtime;
pub mod tool;

pub use builtin::{CodeInterpreterTool, UserCodeInterpreterTool};
pub use runtime::{run_agent, run_agent_stream, AgentConfig, AgentEvent};
pub use tool::{Tool, ToolRegistry, UserSandbox};
