use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chat_sandbox::ExecResult;
use serde_json::Value;
use tokio::net::TcpListener;

#[derive(Clone)]
pub struct FakeSandbox {
    replies: Arc<Mutex<VecDeque<ExecResult>>>,
    fallback: Arc<Mutex<ExecResult>>,
    forced_status: Arc<Mutex<Option<u16>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Default for FakeSandbox {
    fn default() -> Self {
        Self {
            replies: Arc::new(Mutex::new(VecDeque::new())),
            fallback: Arc::new(Mutex::new(ExecResult {
                exit_code: 0,
                stdout: "ok\n".into(),
                ..Default::default()
            })),
            forced_status: Arc::new(Mutex::new(None)),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl FakeSandbox {
    pub async fn start() -> (Self, String) {
        let sandbox = Self::default();
        let app = Router::new()
            .route("/v1/exec", post(exec))
            .with_state(sandbox.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (sandbox, format!("http://{addr}"))
    }

    pub fn push_reply(&self, reply: ExecResult) {
        self.replies.lock().unwrap().push_back(reply);
    }

    pub fn set_fallback(&self, reply: ExecResult) {
        *self.fallback.lock().unwrap() = reply;
    }

    pub fn set_forced_status(&self, status: Option<u16>) {
        *self.forced_status.lock().unwrap() = status;
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

async fn exec(State(s): State<FakeSandbox>, Json(body): Json<Value>) -> Response {
    s.requests.lock().unwrap().push(body);
    if let Some(code) = *s.forced_status.lock().unwrap() {
        return StatusCode::from_u16(code).unwrap().into_response();
    }
    let reply = s
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| s.fallback.lock().unwrap().clone());
    Json(reply).into_response()
}
