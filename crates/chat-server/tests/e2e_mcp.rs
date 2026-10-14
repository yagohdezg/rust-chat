mod common;

use common::{
    delta_text, events_named, node_available, write_echo_server, FakeHttpMcp, Script, Session,
    TestEnvBuilder, Turn,
};
use serde_json::json;

#[tokio::test]
async fn mcp_tool_call_is_routed_to_the_server() {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return;
    }

    let env = TestEnvBuilder::new()
        .env("MCP_ENABLED", "true")
        .script(Script::text("unused").responses(vec![
            vec![
                Turn::ToolCall {
                    name: "mcp__echo__echo".into(),
                    arguments: json!({ "text": "ping" }).to_string(),
                },
                Turn::Finish,
            ],
            vec![Turn::Text("the tool said: ping".into()), Turn::Finish],
        ]))
        .start()
        .await
        .unwrap();

    let session = Session::register(&env.base_url, "mcp@example.com", "password123")
        .await
        .unwrap();
    let provider = session
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();
    let conversation = session.create_conversation("mcp chat").await.unwrap();

    let (command, args) = write_echo_server(env.data_dir.path());
    let server = session
        .post_expect(
            "/api/mcp-servers",
            &json!({ "name": "echo", "transport": "stdio", "command": command, "args": args }),
        )
        .await
        .unwrap();
    assert_eq!(server["name"], "echo");
    assert_eq!(server["enabled"], json!(true));

    let listed = session.get_expect("/api/mcp-servers").await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let events = session
        .chat_sse(conversation, "call the echo tool", None, Some(provider))
        .await
        .unwrap();

    let calls = events_named(&events, "tool_call");
    assert_eq!(calls.len(), 1, "expected one tool_call event: {events:?}");
    assert!(
        calls[0].data.contains("mcp__echo__echo"),
        "tool_call did not name the MCP tool: {}",
        calls[0].data
    );

    let results = events_named(&events, "tool_result");
    assert_eq!(
        results.len(),
        1,
        "expected one tool_result event: {events:?}"
    );
    assert_eq!(results[0].json()["content"], "ping");

    assert_eq!(delta_text(&events), "the tool said: ping");

    let id = server["id"].as_str().unwrap();
    let resp = env
        .http
        .delete(format!("{}/api/mcp-servers/{id}", env.base_url))
        .bearer_auth(&session.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);
    let listed = session.get_expect("/api/mcp-servers").await.unwrap();
    assert!(listed.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn sandboxed_mcp_tool_call_is_bridged_across_the_sandbox() {
    let env = TestEnvBuilder::new()
        .env("MCP_ENABLED", "true")
        .script(Script::text("unused").responses(vec![
            vec![
                Turn::ToolCall {
                    name: "mcp__echo-sandbox__echo".into(),
                    arguments: json!({ "text": "ping" }).to_string(),
                },
                Turn::Finish,
            ],
            vec![Turn::Text("the tool said: ping".into()), Turn::Finish],
        ]))
        .start()
        .await
        .unwrap();

    let session = Session::register(&env.base_url, "sandbox-mcp@example.com", "password123")
        .await
        .unwrap();
    let provider = session
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();
    let conversation = session
        .create_conversation("sandbox mcp chat")
        .await
        .unwrap();

    let server = session
        .post_expect(
            "/api/mcp-servers",
            &json!({
                "name": "echo-sandbox",
                "transport": "stdio",
                "command": "echo-mcp",
                "sandbox": true,
            }),
        )
        .await
        .unwrap();
    assert_eq!(server["sandbox"], json!(true));

    let events = session
        .chat_sse(conversation, "call the echo tool", None, Some(provider))
        .await
        .unwrap();

    let calls = events_named(&events, "tool_call");
    assert_eq!(calls.len(), 1, "expected one tool_call event: {events:?}");
    assert!(
        calls[0].data.contains("mcp__echo-sandbox__echo"),
        "tool_call did not name the sandboxed MCP tool: {}",
        calls[0].data
    );

    let results = events_named(&events, "tool_result");
    assert_eq!(
        results.len(),
        1,
        "expected one tool_result event: {events:?}"
    );
    assert_eq!(results[0].json()["content"], "ping");

    assert_eq!(delta_text(&events), "the tool said: ping");
}

#[tokio::test]
async fn http_mcp_tool_call_is_routed_to_the_server() {
    let env = TestEnvBuilder::new()
        .env("MCP_ENABLED", "true")
        .script(Script::text("unused").responses(vec![
            vec![
                Turn::ToolCall {
                    name: "mcp__echo-http__echo".into(),
                    arguments: json!({ "text": "ping" }).to_string(),
                },
                Turn::Finish,
            ],
            vec![Turn::Text("the tool said: ping".into()), Turn::Finish],
        ]))
        .start()
        .await
        .unwrap();

    let session = Session::register(&env.base_url, "http-mcp@example.com", "password123")
        .await
        .unwrap();
    let provider = session
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();
    let conversation = session.create_conversation("http mcp chat").await.unwrap();

    let mcp = FakeHttpMcp::start().await;
    let server = session
        .post_expect(
            "/api/mcp-servers",
            &json!({ "name": "echo-http", "transport": "http", "url": mcp.url }),
        )
        .await
        .unwrap();
    assert_eq!(server["transport"], "http");

    let events = session
        .chat_sse(conversation, "call the echo tool", None, Some(provider))
        .await
        .unwrap();

    let calls = events_named(&events, "tool_call");
    assert_eq!(calls.len(), 1, "expected one tool_call event: {events:?}");
    assert!(
        calls[0].data.contains("mcp__echo-http__echo"),
        "tool_call did not name the MCP tool: {}",
        calls[0].data
    );

    let results = events_named(&events, "tool_result");
    assert_eq!(
        results.len(),
        1,
        "expected one tool_result event: {events:?}"
    );
    assert_eq!(results[0].json()["content"], "ping");

    assert_eq!(delta_text(&events), "the tool said: ping");
}
