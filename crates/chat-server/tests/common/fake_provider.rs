use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;

#[derive(Clone, Debug)]
pub enum Turn {
    Text(String),
    Reasoning(String),
    ToolCall { name: String, arguments: String },
    Raw(String),
    Finish,
}

#[derive(Clone, Debug)]
pub struct Script {
    pub model: String,
    pub responses: Vec<Vec<Turn>>,
    pub delay_ms: u64,
    pub failures: Vec<(u16, Option<u64>)>,
}

impl Script {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            model: "fake-model".into(),
            responses: vec![vec![Turn::Text(text.into()), Turn::Finish]],
            delay_ms: 0,
            failures: Vec::new(),
        }
    }

    pub fn failures(mut self, failures: Vec<(u16, Option<u64>)>) -> Self {
        self.failures = failures;
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn responses(mut self, responses: Vec<Vec<Turn>>) -> Self {
        self.responses = responses;
        self
    }

    pub fn delay_ms(mut self, delay_ms: u64) -> Self {
        self.delay_ms = delay_ms;
        self
    }
}

#[derive(Clone)]
pub struct FakeProvider {
    script: Arc<Script>,
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl FakeProvider {
    pub async fn start(script: Script) -> (Self, String) {
        let provider = Self {
            script: Arc::new(script),
            calls: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route("/v1/chat/completions", post(chat_completions))
            .route("/v1/models", get(models))
            .with_state(provider.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (provider, format!("http://{addr}/v1"))
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

async fn models(State(p): State<FakeProvider>) -> Json<Value> {
    Json(json!({"data": [{"id": p.script.model, "object": "model"}]}))
}

async fn chat_completions(State(p): State<FakeProvider>, Json(body): Json<Value>) -> Response {
    p.requests.lock().unwrap().push(body);
    let idx = p.calls.fetch_add(1, Ordering::SeqCst);
    if let Some((status, retry_after)) = p.script.failures.get(idx) {
        let mut response = (
            axum::http::StatusCode::from_u16(*status).unwrap(),
            json!({"error": {"message": "scripted failure"}}).to_string(),
        )
            .into_response();
        if let Some(seconds) = retry_after {
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                seconds.to_string().parse().unwrap(),
            );
        }
        return response;
    }
    let turns = p
        .script
        .responses
        .get(idx)
        .or_else(|| p.script.responses.last())
        .cloned()
        .unwrap_or_default();
    let delay = p.script.delay_ms;
    let stream = async_stream::stream! {
        for turn in turns {
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            let event: Result<Event, Infallible> = match turn {
                Turn::Text(text) => Ok(Event::default()
                    .data(json!({"choices": [{"delta": {"content": text}}]}).to_string())),
                Turn::Reasoning(text) => Ok(Event::default().data(
                    json!({"choices": [{"delta": {"reasoning_content": text}}]}).to_string(),
                )),
                Turn::ToolCall { name, arguments } => Ok(Event::default().data(
                    json!({"choices": [{"delta": {"tool_calls": [{
                        "index": 0, "id": "call_1", "type": "function",
                        "function": {"name": name, "arguments": arguments}
                    }]}}]})
                    .to_string(),
                )),
                Turn::Raw(raw) => Ok(Event::default().data(raw)),
                Turn::Finish => Ok(Event::default()
                    .data(json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}).to_string())),
            };
            yield event;
        }
    };
    Sse::new(stream).into_response()
}
