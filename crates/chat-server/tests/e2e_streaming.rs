mod common;

use common::{
    collect_sse, delta_text, events_named, Backend, Script, Session, TestEnvBuilder, Turn,
};
use serde_json::json;
use uuid::Uuid;

const ANSWER: &str = "Hello world";

fn multi_chunk_script() -> Script {
    Script {
        model: "fake-model".into(),
        responses: vec![vec![
            Turn::Text("Hel".into()),
            Turn::Text("lo ".into()),
            Turn::Text("world".into()),
            Turn::Finish,
        ]],
        delay_ms: 0,
    }
}

async fn env_with(script: Script) -> common::TestEnv {
    TestEnvBuilder::new().script(script).start().await.unwrap()
}

async fn one_turn(
    env: &common::TestEnv,
    content: &str,
) -> (Session, Uuid, Uuid, Vec<common::SseEvent>) {
    let session = Session::register(&env.base_url, "a@example.com", "password123")
        .await
        .unwrap();
    let provider_id = session
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();
    let conversation_id = session.create_conversation("streaming chat").await.unwrap();
    let events = session
        .chat_sse(conversation_id, content, None, Some(provider_id))
        .await
        .unwrap();
    let message_id = meta_message_id(&events);
    (session, conversation_id, message_id, events)
}

fn meta_message_id(events: &[common::SseEvent]) -> Uuid {
    let metas = events_named(events, "meta");
    assert_eq!(metas.len(), 1, "expected exactly one meta event");
    metas[0].json()["message_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn event_count(events: &[common::SseEvent], name: &str) -> usize {
    events_named(events, name).len()
}

fn index_of(events: &[common::SseEvent], name: &str) -> Option<usize> {
    events.iter().position(|e| e.event == name)
}

#[tokio::test]
async fn stream_emits_meta_first_and_done_last() {
    let env = env_with(Script::text(ANSWER)).await;
    let (_session, _conversation, _message_id, events) = one_turn(&env, "hi").await;

    assert!(!events.is_empty(), "expected at least the meta event");
    assert_eq!(events[0].event, "meta");
    assert!(events.first().unwrap().json()["message_id"].is_string());
    assert_eq!(
        events.first().unwrap().json()["resume_path"],
        json!(format!("/api/messages/{}/stream", meta_message_id(&events)))
    );

    assert_eq!(delta_text(&events), ANSWER);
    let mut last: Option<usize> = None;
    for event in events_named(&events, "delta") {
        let id: usize = event
            .id
            .as_ref()
            .expect("delta must carry an SSE id")
            .parse()
            .expect("delta id must parse to usize");
        if let Some(prev) = last {
            assert!(
                id > prev,
                "delta ids must strictly increase: {prev} -> {id}"
            );
        }
        last = Some(id);
    }

    assert_eq!(event_count(&events, "done"), 1, "exactly one done");
    assert_eq!(events.last().unwrap().event, "done", "done must be last");
}

#[tokio::test]
async fn stream_recombines_multiple_text_chunks() {
    let env = env_with(multi_chunk_script()).await;
    let (_session, _conversation, _message_id, events) = one_turn(&env, "hi").await;

    assert_eq!(delta_text(&events), "Hello world");

    let deltas = events_named(&events, "delta");
    assert!(deltas.len() >= 2, "expected multiple delta events");

    let ids: Vec<usize> = deltas
        .iter()
        .map(|e| e.id.as_ref().unwrap().parse().unwrap())
        .collect();
    assert!(
        ids.windows(2).all(|w| w[0] < w[1]),
        "ids not increasing: {ids:?}"
    );
    assert_eq!(*ids.last().unwrap(), ANSWER.len());

    assert_eq!(event_count(&events, "done"), 1);
    assert_eq!(events.last().unwrap().event, "done");
}

#[tokio::test]
async fn stream_emits_error_then_done_when_provider_breaks() {
    let script = Script {
        model: "fake-model".into(),
        responses: vec![vec![Turn::Raw("not-json".into()), Turn::Finish]],
        delay_ms: 0,
    };
    let env = env_with(script).await;
    let (_session, _conversation, _message_id, events) = one_turn(&env, "hi").await;

    assert!(
        event_count(&events, "error") >= 1,
        "expected at least one error event, got: {:?}",
        events.iter().map(|e| &e.event).collect::<Vec<_>>()
    );
    assert_eq!(event_count(&events, "done"), 1, "exactly one done");

    let done_index = index_of(&events, "done").unwrap();
    for (i, event) in events.iter().enumerate() {
        if event.event == "error" {
            assert!(i < done_index, "every error must precede done");
        }
    }
    assert_eq!(events.last().unwrap().event, "done", "done must be last");
}

const CODE_ARGS: &str = r#"{"language":"python","code":"print(1)"}"#;
const FINAL: &str = "all done";

fn tool_script() -> Script {
    Script {
        model: "fake-model".into(),
        responses: vec![
            vec![
                Turn::ToolCall {
                    name: "execute_code".into(),
                    arguments: CODE_ARGS.into(),
                },
                Turn::Finish,
            ],
            vec![Turn::Text(FINAL.into()), Turn::Finish],
        ],
        delay_ms: 0,
    }
}

#[tokio::test]
async fn stream_forwards_tool_call_and_result_before_final_deltas() {
    let env = TestEnvBuilder::new()
        .sandbox_tool(true)
        .script(tool_script())
        .start()
        .await
        .unwrap();
    let (_session, _conversation, _message_id, events) = one_turn(&env, "run some code").await;

    let tool_call_index = index_of(&events, "tool_call").expect("expected a tool_call event");
    let tool_result_index = index_of(&events, "tool_result").expect("expected a tool_result event");
    assert!(
        tool_call_index < tool_result_index,
        "tool_call must precede tool_result"
    );

    let results = events_named(&events, "tool_result");
    assert_eq!(results.len(), 1);
    let data = results[0].json();
    assert!(data["tool_call_id"].is_string());
    assert_eq!(data["name"], json!("execute_code"));
    assert!(data["content"].is_string());

    let first_delta = index_of(&events, "delta").expect("expected a final delta");
    let done_index = index_of(&events, "done").expect("expected done");
    assert!(
        first_delta > tool_result_index,
        "final deltas must follow the tool result"
    );
    assert!(done_index > first_delta, "done must be last");
    assert_eq!(delta_text(&events), FINAL);
    assert_eq!(event_count(&events, "done"), 1);

    let requests = env.sandbox.requests();
    assert_eq!(requests.len(), 1, "expected exactly one /v1/exec POST");
    assert_eq!(requests[0]["language"], json!("python"));
    assert_eq!(requests[0]["code"], json!("print(1)"));
}

#[tokio::test]
async fn resume_stream_replays_only_the_tail() {
    let env = env_with(Script::text(ANSWER)).await;
    let (session, _conversation, message_id, events) = one_turn(&env, "hi").await;
    assert_eq!(delta_text(&events), ANSWER);

    let resp = session
        .http
        .get(format!("{}/api/messages/{message_id}/stream", session.base))
        .bearer_auth(&session.token)
        .header("Last-Event-ID", "5")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let resumed = collect_sse(resp).await;

    assert_eq!(resumed[0].event, "meta");
    assert_eq!(resumed[0].json()["resumed"], json!(true));
    assert_eq!(
        delta_text(&resumed),
        &ANSWER[5..],
        "resume must return only bytes past the offset"
    );
    assert_eq!(event_count(&resumed, "done"), 1);
    assert_eq!(resumed.last().unwrap().event, "done");
}

#[tokio::test]
async fn resume_stream_without_last_event_id_replays_whole_reply() {
    let env = env_with(Script::text(ANSWER)).await;
    let (session, _conversation, message_id, events) = one_turn(&env, "hi").await;
    assert_eq!(delta_text(&events), ANSWER);

    let resp = session
        .http
        .get(format!("{}/api/messages/{message_id}/stream", session.base))
        .bearer_auth(&session.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let replayed = collect_sse(resp).await;

    assert_eq!(replayed[0].event, "meta");
    assert_eq!(replayed[0].json()["resumed"], json!(true));
    assert_eq!(delta_text(&replayed), ANSWER);
    assert_eq!(event_count(&replayed, "done"), 1);
    assert_eq!(replayed.last().unwrap().event, "done");
}

#[tokio::test]
async fn stream_runs_on_postgres() {
    if !common::postgres_available() {
        eprintln!("skipping: no container runtime available");
        return;
    }
    let env = TestEnvBuilder::new()
        .backend(Backend::Postgres)
        .script(Script::text(ANSWER))
        .start()
        .await
        .unwrap();

    let (session, _conversation, message_id, events) = {
        let session = Session::register(&env.base_url, "a@example.com", "password123")
            .await
            .unwrap();
        let provider_id = session
            .create_provider("fake", &env.provider_url)
            .await
            .unwrap();
        let conversation_id = session.create_conversation("pg streaming").await.unwrap();
        let events = session
            .chat_sse(conversation_id, "hi", None, Some(provider_id))
            .await
            .unwrap();
        let message_id = meta_message_id(&events);
        (session, conversation_id, message_id, events)
    };

    assert_eq!(events[0].event, "meta");
    assert_eq!(delta_text(&events), ANSWER);
    assert_eq!(event_count(&events, "done"), 1);
    assert_eq!(events.last().unwrap().event, "done");

    let resp = session
        .http
        .get(format!("{}/api/messages/{message_id}/stream", session.base))
        .bearer_auth(&session.token)
        .send()
        .await
        .unwrap();
    let replayed = collect_sse(resp).await;
    assert_eq!(delta_text(&replayed), ANSWER);
    assert_eq!(event_count(&replayed, "done"), 1);
}

const REASONING: &str = "let me think";

fn reasoning_script() -> Script {
    Script {
        model: "fake-model".into(),
        responses: vec![vec![
            Turn::Reasoning(REASONING.into()),
            Turn::Text(ANSWER.into()),
            Turn::Finish,
        ]],
        delay_ms: 0,
    }
}

fn assistant_message(messages: &serde_json::Value) -> &serde_json::Value {
    messages
        .as_array()
        .expect("messages must be an array")
        .iter()
        .find(|message| message["role"] == json!("assistant"))
        .expect("expected an assistant message")
}

#[tokio::test]
async fn plain_stream_emits_thinking_before_delta_and_persists_reasoning() {
    let env = env_with(reasoning_script()).await;
    let (session, conversation_id, _message_id, events) = one_turn(&env, "hi").await;

    let thinking = events_named(&events, "thinking");
    assert!(!thinking.is_empty(), "expected at least one thinking event");
    let reasoning: String = thinking.iter().map(|event| event.text()).collect();
    assert_eq!(reasoning, REASONING);

    let first_thinking = index_of(&events, "thinking").unwrap();
    let first_delta = index_of(&events, "delta").unwrap();
    assert!(
        first_thinking < first_delta,
        "thinking must precede the first delta"
    );

    assert_eq!(delta_text(&events), ANSWER);
    assert_eq!(event_count(&events, "done"), 1, "exactly one done");
    assert_eq!(events.last().unwrap().event, "done", "done must be last");

    let messages = session
        .get_expect(&format!("/api/conversations/{conversation_id}/messages"))
        .await
        .unwrap();
    assert_eq!(assistant_message(&messages)["reasoning"], json!(REASONING));
}

#[tokio::test]
async fn non_reasoning_stream_emits_no_thinking() {
    let env = env_with(Script::text(ANSWER)).await;
    let (session, conversation_id, _message_id, events) = one_turn(&env, "hi").await;

    assert_eq!(
        event_count(&events, "thinking"),
        0,
        "a non-reasoning model must emit no thinking events"
    );
    assert_eq!(delta_text(&events), ANSWER);

    let messages = session
        .get_expect(&format!("/api/conversations/{conversation_id}/messages"))
        .await
        .unwrap();
    let reasoning = &assistant_message(&messages)["reasoning"];
    assert!(
        reasoning.is_null() || reasoning == &json!(""),
        "expected no reasoning, got {reasoning}"
    );
}

#[tokio::test]
async fn agent_stream_emits_thinking_and_persists_reasoning() {
    let env = env_with(reasoning_script()).await;
    let session = Session::register(&env.base_url, "a@example.com", "password123")
        .await
        .unwrap();
    let provider_id = session
        .create_provider("fake", &env.provider_url)
        .await
        .unwrap();
    let agent = session
        .post_expect(
            "/api/agents",
            &json!({"name": "reasoner", "provider_id": provider_id, "model": "fake-model"}),
        )
        .await
        .unwrap();
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();
    let conversation_id: Uuid = session
        .post_expect(
            "/api/conversations",
            &json!({"title": "t", "agent_id": agent_id}),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let events = session
        .chat_sse(conversation_id, "hi", None, None)
        .await
        .unwrap();

    let thinking = events_named(&events, "thinking");
    assert!(!thinking.is_empty(), "expected at least one thinking event");
    let reasoning: String = thinking.iter().map(|event| event.text()).collect();
    assert_eq!(reasoning, REASONING);
    assert_eq!(event_count(&events, "done"), 1, "exactly one done");
    assert_eq!(events.last().unwrap().event, "done", "done must be last");

    let messages = session
        .get_expect(&format!("/api/conversations/{conversation_id}/messages"))
        .await
        .unwrap();
    assert_eq!(assistant_message(&messages)["reasoning"], json!(REASONING));
}
