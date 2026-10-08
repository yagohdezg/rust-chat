use chat_core::{ChatError, Result};
use chat_store::Embedder;
use serde_json::{json, Value};

/// Embedder backed by an OpenAI-compatible `/embeddings` endpoint.
pub struct OpenAiEmbedder {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    dimensions: usize,
}

impl OpenAiEmbedder {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimensions: usize,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            model: model.into(),
            dimensions,
        }
    }
}

#[async_trait::async_trait]
impl Embedder for OpenAiEmbedder {
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let body = json!({ "model": self.model, "input": texts });
        let response = self
            .http
            .post(format!("{}/embeddings", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ChatError::Internal(anyhow::anyhow!("embedding request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(ChatError::Internal(anyhow::anyhow!(
                "embedding endpoint returned {status}: {text}"
            )));
        }

        let value: Value = response
            .json()
            .await
            .map_err(|e| ChatError::Internal(anyhow::anyhow!("invalid embedding response: {e}")))?;

        let data = value.get("data").and_then(Value::as_array).ok_or_else(|| {
            ChatError::Internal(anyhow::anyhow!("embedding response missing `data`"))
        })?;

        // Preserve the order of the input even if the API returns out of order.
        let mut indexed: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());
        for item in data {
            let index = item.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let embedding = item
                .get("embedding")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    ChatError::Internal(anyhow::anyhow!("embedding item missing vector"))
                })?
                .iter()
                .map(|n| n.as_f64().unwrap_or(0.0) as f32)
                .collect();
            indexed.push((index, embedding));
        }
        indexed.sort_by_key(|(index, _)| *index);

        Ok(indexed
            .into_iter()
            .map(|(_, embedding)| embedding)
            .collect())
    }
}
