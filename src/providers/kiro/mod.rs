pub mod auth;
pub mod client;
pub mod models;
pub mod strategies;
pub mod stores;

use std::pin::Pin;
use std::sync::Arc;
use futures::Stream;
use futures::stream::StreamExt;
use serde_json::Value;
use tracing::debug;

use crate::providers::pricing::ModelPricing;
use crate::providers::transform::kiro::KiroTransformer;
use crate::providers::transformer::{ProviderResponseMeta, ProviderTransformer};
use crate::providers::types::*;
use crate::providers::{LlmProvider, ProviderError};

pub use self::auth::{KiroTokenProvider, KiroAuthManager, AutoDetectProvider};
pub use self::client::{KiroClient, machine_fingerprint};

pub struct KiroProvider {
    client: Arc<KiroClient>,
    transformer: KiroTransformer,
}

impl KiroProvider {
    pub fn new(client: KiroClient) -> Self {
        Self {
            client: Arc::new(client),
            transformer: KiroTransformer::new(),
        }
    }
}

impl LlmProvider for KiroProvider {
    fn id(&self) -> &str {
        "kiro"
    }

    fn name(&self) -> &str {
        "Kiro (Amazon Q)"
    }

    fn models(&self) -> Vec<String> {
        self.transformer.supported_models()
    }

    fn supports_model(&self, model: &str) -> bool {
        self.transformer.supports_model(model)
    }

    fn chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>>
    {
        let request = request.clone();
        Box::pin(async move {
            let body: Value = self.transformer.transform_request(&request)?;
            debug!(body = %body, "Kiro request body");

            let sse_body = self.client.send_request(&body).await?;

            let mut state = self.transformer.new_stream_state(&request.model);
            let mut final_chunk: Option<ChatChunk> = None;
            for line in sse_body.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let data = if let Some(d) = line.strip_prefix("data: ") {
                    d
                } else {
                    line
                };
                if data == "[DONE]" {
                    continue;
                }
                if let Ok(Some(chunk)) = state.process_event(data) {
                    final_chunk = Some(chunk);
                }
            }

            if let Some(chunk) = final_chunk {
                let meta = ProviderResponseMeta {
                    provider: "kiro".to_string(),
                    model: request.model.clone(),
                    created: chrono::Utc::now().timestamp(),
                    ..Default::default()
                };
                Ok(ChatResponse {
                    id: chunk.id,
                    object: "chat.completion".to_string(),
                    created: meta.created,
                    model: meta.model,
                    choices: chunk
                        .choices
                        .into_iter()
                        .map(|c| Choice {
                            index: c.index,
                            message: ResponseMessage {
                                role: "assistant".to_string(),
                                content: c.delta.content,
                                reasoning_content: c.delta.reasoning_content,
                                tool_calls: c.delta.tool_calls,
                            },
                            finish_reason: c.finish_reason,
                        })
                        .collect(),
                    usage: chunk.usage.unwrap_or_default(),
                })
            } else {
                Err(ProviderError::ResponseParsing(
                    "Kiro SSE response contained no usable events".to_string(),
                ))
            }
        })
    }

    fn stream_chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>,
                        ProviderError,
                    >,
                > + Send
                + '_,
        >,
    > {
        let request = request.clone();
        Box::pin(async move {
            let body: Value = self.transformer.transform_request(&request)?;
            let sse_stream = self.client.send_request_stream(&body).await?;

            let model = request.model.clone();
            let mut stream_state = self.transformer.new_stream_state(&model);

            let event_stream = sse_stream.filter_map(move |result| {
                let chunk = match result {
                    Ok(data) => match stream_state.process_event(&data) {
                        Ok(Some(chunk)) => Some(Ok(chunk)),
                        Ok(None) => None,
                        Err(e) => Some(Err(e)),
                    },
                    Err(e) => Some(Err(e)),
                };
                async move { chunk }
            });

            Ok(Box::pin(event_stream)
                as Pin<
                    Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>,
                >)
        })
    }

    fn health_check(&self) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(async move { self.client.health_check().await })
    }

    fn pricing(&self) -> Vec<ModelPricing> {
        crate::providers::cost::CostCalculator::all()
            .into_iter()
            .filter(|p| p.provider == "kiro")
            .collect()
    }
}
