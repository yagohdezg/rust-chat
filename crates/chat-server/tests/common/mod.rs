//! Shared helpers for the end-to-end suites. Each test binary links this module
//! whole but uses a subset of it, so unused items are expected here.
#![allow(dead_code)]
#![allow(unused_imports)]

pub mod api;
pub mod fake_http_mcp;
pub mod fake_mcp;
pub mod fake_provider;
pub mod fake_sandbox;
pub mod server;

pub use api::{collect_sse, delta_text, events_named, Session, SseEvent};
pub use chat_sandbox::ExecResult;
pub use fake_http_mcp::FakeHttpMcp;
pub use fake_mcp::{node_available, write_echo_server};
pub use fake_provider::{FakeAnthropic, FakeProvider, Script, Turn};
pub use fake_sandbox::FakeSandbox;
pub use server::{postgres_available, Backend, TestEnv, TestEnvBuilder};
