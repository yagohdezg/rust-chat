//! Failover wrapper around two providers.
//!
//! The primary provider runs first; when it refuses a request outright (after
//! its own retries) the request is replayed against a secondary provider, with
//! the model swapped for the configured fallback model. Failover only covers a
//! request that never started streaming — once chunks have been handed to the
//! caller there is no way to un-send them, so a mid-stream error is reported as
//! is.

use std::sync::Arc;

use async_trait::async_trait;

use crate::{ChatRequest, ChatStream, LlmProvider, ModelInfo, ProviderError};

/// Run `primary` and fall back to `secondary` when the primary refuses the
/// request.
pub struct FailoverProvider {
    primary: Arc<dyn LlmProvider>,
    secondary: Arc<dyn LlmProvider>,
    model: Option<String>,
}

impl FailoverProvider {
    /// Wrap `primary`, replaying a refused request against `secondary`. When
    /// `model` is set the replayed request asks the secondary for that model.
    pub fn new(
        primary: Arc<dyn LlmProvider>,
        secondary: Arc<dyn LlmProvider>,
        model: Option<String>,
    ) -> Self {
        Self {
            primary,
            secondary,
            model,
        }
    }
}

#[async_trait]
impl LlmProvider for FailoverProvider {
    fn name(&self) -> &str {
        self.primary.name()
    }

    async fn stream(&self, mut request: ChatRequest) -> Result<ChatStream, ProviderError> {
        match self.primary.stream(request.clone()).await {
            Ok(stream) => Ok(stream),
            Err(primary) => {
                if let Some(model) = &self.model {
                    request.model = model.clone();
                }
                self.secondary.stream(request).await.map_err(|secondary| {
                    ProviderError::Other(format!(
                        "primary provider failed ({primary}); failover provider failed ({secondary})"
                    ))
                })
            }
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.primary.list_models().await
    }
}
