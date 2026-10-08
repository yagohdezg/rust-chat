//! Agent runtime: provider streaming + a tool-call execution loop.

pub mod builtin;
pub mod runtime;
pub mod tool;

pub use builtin::CodeInterpreterTool;
pub use runtime::{run_agent, AgentConfig};
pub use tool::{Tool, ToolRegistry};
