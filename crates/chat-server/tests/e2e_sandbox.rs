mod common;

use base64::Engine as _;
use chat_sandbox::{ExecResult, SandboxFile};
use common::{delta_text, events_named, Backend, Script, Session, TestEnv, TestEnvBuilder, Turn};
use serde_json::json;
use uuid::Uuid;

const CODE_ARGS: &str = r#"{"language":"python","code":"print(1)"}"#;
const ANSWER: &str = "all done";

fn script_with_tool(tool_args: &str, answer: &str) -> Script {
    Script {
        model: "fake-model".into(),
        responses: vec![
            vec![
                Turn::ToolCall {
                    name: "execute_code".into(),
                    arguments: tool_args.to_string(),
                },
                Turn::Finish,
            ],
            vec![Turn::Text(answer.to_string()), Turn::Finish],
        ],
        delay_ms: 0,
    }
}

async fn env_with_tool(tool_args: &str, answer: &str) -> TestEnv {
    TestEnvBuilder::new()
        .sandbox_tool(true)
        .script(script_with_tool(tool_args, answer))
        .start()
        .await
        .unwrap()
}

async fn run_chat(env: &TestEnv) -> (Session, Uuid, Vec<common::SseEvent>) {
    let session = Session::register(&env.base_url, "a@example.com", "password123")
        .await
        .unwrap();
    let provider_id = session
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();
    let conversation_id = session.create_conversation("sandbox chat").await.unwrap();
    let events = session
        .chat_sse(conversation_id, "run some code", None, Some(provider_id))
        .await
        .unwrap();
    (session, conversation_id, events)
}

fn tool_result_content(events: &[common::SseEvent]) -> String {
    let results = events_named(events, "tool_result");
    assert_eq!(results.len(), 1, "expected exactly one tool_result");
    results[0].json()["content"].as_str().unwrap().to_string()
}

fn assert_turn_completed(events: &[common::SseEvent]) {
    assert_eq!(
        events_named(events, "done").len(),
        1,
        "expected exactly one terminal done event; got {:?}",
        events.iter().map(|e| e.event.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(delta_text(events), ANSWER);
    assert!(events_named(events, "error").is_empty());
}

#[tokio::test]
async fn sandbox_exec_success_reports_stdout() {
    let env = env_with_tool(CODE_ARGS, ANSWER).await;
    env.sandbox.push_reply(ExecResult {
        exit_code: 0,
        stdout: "ok\n".into(),
        stderr: String::new(),
        ..Default::default()
    });

    let (_session, _conversation, events) = run_chat(&env).await;

    assert_eq!(events_named(&events, "tool_call").len(), 1);
    let content = tool_result_content(&events);
    assert!(content.contains("exit_code=0"), "{content}");
    assert!(content.contains("ok"), "{content}");

    let requests = env.sandbox.requests();
    assert_eq!(requests.len(), 1, "expected exactly one /v1/exec POST");
    assert_eq!(requests[0]["language"], json!("python"));
    assert_eq!(requests[0]["code"], json!("print(1)"));

    assert_turn_completed(&events);
}

#[tokio::test]
async fn sandbox_exec_failure_reports_exit_code_and_stderr() {
    let env = env_with_tool(CODE_ARGS, ANSWER).await;
    env.sandbox.push_reply(ExecResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: "boom\n".into(),
        ..Default::default()
    });

    let (_session, _conversation, events) = run_chat(&env).await;

    let content = tool_result_content(&events);
    assert!(content.contains("exit_code=1"), "{content}");
    assert!(content.contains("boom"), "{content}");
    assert_turn_completed(&events);
}

#[tokio::test]
async fn sandbox_captures_stdout_and_stderr() {
    let env = env_with_tool(CODE_ARGS, ANSWER).await;
    env.sandbox.push_reply(ExecResult {
        exit_code: 0,
        stdout: "to stdout\n".into(),
        stderr: "to stderr\n".into(),
        ..Default::default()
    });

    let (_session, _conversation, events) = run_chat(&env).await;

    let content = tool_result_content(&events);
    assert!(content.contains("to stdout"), "{content}");
    assert!(content.contains("to stderr"), "{content}");
    assert_turn_completed(&events);
}

#[tokio::test]
async fn sandbox_output_files_are_reported_and_saved() {
    let env = env_with_tool(CODE_ARGS, ANSWER).await;
    let payload = base64::engine::general_purpose::STANDARD.encode(b"hello output");
    env.sandbox.push_reply(ExecResult {
        exit_code: 0,
        stdout: "wrote file\n".into(),
        stderr: String::new(),
        files: vec![SandboxFile {
            name: "result.txt".into(),
            content_b64: payload,
        }],
        ..Default::default()
    });

    let (session, conversation_id, events) = run_chat(&env).await;

    let content = tool_result_content(&events);
    assert!(content.contains("result.txt"), "{content}");

    let files = session
        .get_expect(&format!("/api/conversations/{conversation_id}/files"))
        .await
        .unwrap();
    let names: Vec<&str> = files
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["filename"].as_str())
        .collect();
    assert!(names.contains(&"result.txt"), "files: {names:?}");
    assert_turn_completed(&events);
}

#[tokio::test]
async fn sandbox_timeout_is_reported() {
    let env = env_with_tool(CODE_ARGS, ANSWER).await;
    env.sandbox.push_reply(ExecResult {
        exit_code: 124,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: true,
        ..Default::default()
    });

    let (_session, _conversation, events) = run_chat(&env).await;

    let content = tool_result_content(&events);
    assert!(content.contains("timed_out=true"), "{content}");
    assert_turn_completed(&events);
}

#[tokio::test]
async fn sandbox_unavailable_fails_the_tool_and_ends_the_turn() {
    let env = env_with_tool(CODE_ARGS, ANSWER).await;
    env.sandbox.set_forced_status(Some(503));

    let (_session, _conversation, events) = run_chat(&env).await;

    let content = tool_result_content(&events);
    assert!(
        content.contains("failed") && content.contains("503"),
        "tool_result should report the sandbox failure: {content}"
    );
    assert_eq!(
        events_named(&events, "done").len(),
        1,
        "the turn must still terminate with exactly one done"
    );
    env.sandbox.set_forced_status(None);
}

#[tokio::test]
async fn sandbox_contract_runs_on_postgres() {
    if !common::postgres_available() {
        eprintln!("skipping: no container runtime available");
        return;
    }
    let env = TestEnvBuilder::new()
        .backend(Backend::Postgres)
        .sandbox_tool(true)
        .script(script_with_tool(CODE_ARGS, ANSWER))
        .start()
        .await
        .unwrap();
    env.sandbox.push_reply(ExecResult {
        exit_code: 0,
        stdout: "ok\n".into(),
        stderr: String::new(),
        ..Default::default()
    });

    let (_session, _conversation, events) = run_chat(&env).await;

    let content = tool_result_content(&events);
    assert!(content.contains("exit_code=0"), "{content}");
    assert!(content.contains("ok"), "{content}");
    assert_eq!(env.sandbox.requests().len(), 1);
    assert_turn_completed(&events);
}

#[tokio::test]
async fn sandbox_exec_runs_against_real_sandboxd_when_available() {
    if !std::path::Path::new("/dev/kvm").exists() {
        eprintln!("skipping: no /dev/kvm available");
        return;
    }
    let Ok(sandboxd_url) = std::env::var("RUSTCHAT_E2E_SANDBOXD_URL") else {
        eprintln!("skipping: RUSTCHAT_E2E_SANDBOXD_URL is not set");
        return;
    };
    if sandboxd_url.trim().is_empty() {
        eprintln!("skipping: RUSTCHAT_E2E_SANDBOXD_URL is empty");
        return;
    }

    let env = TestEnvBuilder::new()
        .sandbox_tool(true)
        .script(script_with_tool(
            r#"{"language":"python","code":"print('ok')"}"#,
            ANSWER,
        ))
        .env("SANDBOXD_URL", &sandboxd_url)
        .start()
        .await
        .unwrap();

    let (_session, _conversation, events) = run_chat(&env).await;

    let content = tool_result_content(&events);
    assert!(content.contains("exit_code=0"), "{content}");
    assert!(content.contains("ok"), "{content}");
    assert_turn_completed(&events);
}
