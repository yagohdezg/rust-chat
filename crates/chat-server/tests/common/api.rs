use anyhow::{anyhow, Result};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
    pub id: Option<String>,
}

#[derive(Clone)]
pub struct Session {
    pub http: reqwest::Client,
    pub base: String,
    pub token: String,
    pub user_id: Uuid,
    pub email: String,
    pub role: String,
}

impl Session {
    pub async fn register(base: &str, email: &str, password: &str) -> Result<Self> {
        let http = reqwest::Client::new();
        let resp = http
            .post(format!("{base}/api/auth/register"))
            .json(&json!({"email": email, "password": password}))
            .send()
            .await?;
        Self::from_response(http, base, email, resp).await
    }

    pub async fn login(base: &str, email: &str, password: &str) -> Result<Self> {
        let http = reqwest::Client::new();
        let resp = http
            .post(format!("{base}/api/auth/login"))
            .json(&json!({"email": email, "password": password}))
            .send()
            .await?;
        Self::from_response(http, base, email, resp).await
    }

    async fn from_response(
        http: reqwest::Client,
        base: &str,
        email: &str,
        resp: reqwest::Response,
    ) -> Result<Self> {
        let status = resp.status();
        let body: Value = resp.json().await?;
        if !status.is_success() {
            return Err(anyhow!("auth failed ({status}): {body}"));
        }
        Ok(Self {
            http,
            base: base.to_string(),
            token: body["token"].as_str().unwrap().to_string(),
            user_id: body["user"]["id"].as_str().unwrap().parse()?,
            email: email.to_string(),
            role: body["user"]["role"].as_str().unwrap().to_string(),
        })
    }

    pub async fn get(&self, path: &str) -> Result<reqwest::Response> {
        Ok(self
            .http
            .get(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await?)
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<reqwest::Response> {
        Ok(self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?)
    }

    pub async fn post_expect(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self.post(path, body).await?;
        let status = resp.status();
        let value: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(anyhow!("POST {path} failed ({status}): {value}"));
        }
        Ok(value)
    }

    pub async fn patch_expect(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self
            .http
            .patch(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let value: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(anyhow!("PATCH {path} failed ({status}): {value}"));
        }
        Ok(value)
    }

    pub async fn get_expect(&self, path: &str) -> Result<Value> {
        let resp = self.get(path).await?;
        let status = resp.status();
        let value: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(anyhow!("GET {path} failed ({status}): {value}"));
        }
        Ok(value)
    }

    pub async fn create_conversation(&self, title: &str) -> Result<Uuid> {
        let value = self
            .post_expect("/api/conversations", &json!({"title": title}))
            .await?;
        Ok(value["id"].as_str().unwrap().parse()?)
    }

    pub async fn create_provider(&self, name: &str, base_url: &str) -> Result<Uuid> {
        let value = self
            .post_expect(
                "/api/providers",
                &json!({"name": name, "base_url": base_url, "api_key": "fake-key"}),
            )
            .await?;
        Ok(value["id"].as_str().unwrap().parse()?)
    }

    pub async fn chat(
        &self,
        conversation_id: Uuid,
        content: &str,
        model: Option<&str>,
        provider_id: Option<Uuid>,
    ) -> Result<reqwest::Response> {
        let mut body = json!({"conversation_id": conversation_id, "content": content});
        if let Some(model) = model {
            body["model"] = json!(model);
        }
        if let Some(provider_id) = provider_id {
            body["provider_id"] = json!(provider_id);
        }
        Ok(self
            .http
            .post(format!("{}/api/chat", self.base))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await?)
    }

    pub async fn chat_sse(
        &self,
        conversation_id: Uuid,
        content: &str,
        model: Option<&str>,
        provider_id: Option<Uuid>,
    ) -> Result<Vec<SseEvent>> {
        let resp = self
            .chat(conversation_id, content, model, provider_id)
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("chat failed ({status}): {body}"));
        }
        Ok(collect_sse(resp).await)
    }
}

impl SseEvent {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.data).unwrap_or(Value::Null)
    }

    pub fn text(&self) -> String {
        self.json()["text"].as_str().unwrap_or_default().to_string()
    }
}

pub async fn collect_sse(response: reqwest::Response) -> Vec<SseEvent> {
    let mut stream = response.bytes_stream().eventsource();
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => events.push(SseEvent {
                event: event.event,
                data: event.data,
                id: if event.id.is_empty() {
                    None
                } else {
                    Some(event.id)
                },
            }),
            Err(_) => break,
        }
    }
    events
}

pub fn delta_text(events: &[SseEvent]) -> String {
    events
        .iter()
        .filter(|event| event.event == "delta")
        .map(|event| event.data.clone())
        .collect()
}

pub fn events_named<'a>(events: &'a [SseEvent], name: &str) -> Vec<&'a SseEvent> {
    events.iter().filter(|event| event.event == name).collect()
}
