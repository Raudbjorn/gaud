//! GitHub Copilot Provider
//!
//! Routes requests to the GitHub Copilot Chat Completions API, which natively
//! accepts OpenAI-format payloads. Minimal conversion is needed.

use std::collections::VecDeque;
use std::pin::Pin;

use futures::Stream;

use futures::stream::{self, StreamExt};
use crate::net::HttpClient;

use crate::providers::pricing::ModelPricing;
use crate::providers::transform::util::{detect_context_window_error, parse_rate_limit_headers};
use crate::providers::transform::{CopilotTransformer, SseEvent, SseParser};
use crate::providers::transformer::{ProviderResponseMeta, ProviderTransformer};
use crate::providers::types::*;
use crate::auth::TokenProvider;
use crate::providers::traits::LlmProvider;
use crate::providers::ProviderError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const API_ENDPOINT: &str = "https://api.githubcopilot.com/chat/completions";

const SUPPORTED_MODELS: &[&str] = &["gpt-4o", "gpt-4-turbo", "o1", "o3-mini"];

// ---------------------------------------------------------------------------
// Copilot Provider
// ---------------------------------------------------------------------------

/// LLM provider that communicates with the GitHub Copilot Chat API.
pub struct CopilotProvider<T: TokenProvider> {
    http: HttpClient,
    tokens: std::sync::Arc<T>,
}

impl<T: TokenProvider + 'static> CopilotProvider<T> {
    /// Create a new Copilot provider backed by the given token storage.
    pub fn new(tokens: std::sync::Arc<T>) -> Self {
        Self {
            http: HttpClient::new().expect("Failed to create HTTP client"),
            tokens,
        }
    }

    /// Retrieve an access token or return an error.
    async fn get_token(&self) -> Result<String, ProviderError> {
        self.tokens
            .get_token("copilot")
            .await
            .map_err(|e| match e {
                crate::auth::error::AuthError::TokenNotFound(_) => {
                    ProviderError::NoToken { provider: "copilot".into() }
                }
                _ => ProviderError::Authentication {
                    provider: "copilot".into(),
                    message: e.to_string(),
                    retry_count: 0,
                    max_retries: 0,
                },
            })
    }
}

// ---------------------------------------------------------------------------
// LlmProvider implementation
// ---------------------------------------------------------------------------

impl<T: TokenProvider + 'static> LlmProvider for CopilotProvider<T> {
    fn id(&self) -> &str {
        "copilot"
    }

    fn name(&self) -> &str {
        "GitHub Copilot"
    }

    fn models(&self) -> Vec<String> {
        SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect()
    }

    fn supports_model(&self, model: &str) -> bool {
        SUPPORTED_MODELS.iter().any(|m| *m == model)
    }

    fn chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>>
    {
        let request = request.clone();
        Box::pin(async move {
            let token = self.get_token().await?;
            let transformer = CopilotTransformer::new();
            let body = transformer.transform_request(&request)?;

            let resp = self
                .http
                .inner()
                .post(API_ENDPOINT)
                .bearer_auth(&token)
                .header("content-type", "application/json")
                .header("editor-version", "gaud/0.1.0")
                .header("copilot-integration-id", "gaud")
                .json(&body)
                .send()
                .await?;

            let status = resp.status();
            let resp_headers: Vec<(String, String)> = resp
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();

            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                let code = status.as_u16();

                if let Some(ctx_err) = detect_context_window_error(code, &text, "copilot") {
                    return Err(ctx_err);
                }

                if code == 429 {
                    let (retry_after, _) = parse_rate_limit_headers(&resp_headers, "copilot");
                    return Err(ProviderError::RateLimited {
                        retry_after_secs: retry_after.map(|d| d.as_secs()).unwrap_or(60),
                        retry_after,
                    });
                }
                return Err(ProviderError::Api {
                    status: code,
                    message: text,
                });
            }

            let (_, rate_limit_headers) = parse_rate_limit_headers(&resp_headers, "copilot");
            let response_json: serde_json::Value = resp.json().await?;
            let meta = ProviderResponseMeta {
                provider: "copilot".into(),
                model: request.model.clone(),
                created: chrono::Utc::now().timestamp(),
                rate_limit_headers,
                ..Default::default()
            };
            transformer.transform_response(response_json, &meta)
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
            let token = self.get_token().await?;
            let transformer = CopilotTransformer::new();
            let mut body = transformer.transform_request(&request)?;
            body["stream"] = serde_json::json!(true);

            let resp = self
                .http
                .inner()
                .post(API_ENDPOINT)
                .bearer_auth(&token)
                .header("content-type", "application/json")
                .header("editor-version", "gaud/0.1.0")
                .header("copilot-integration-id", "gaud")
                .json(&body)
                .send()
                .await?;

            let status = resp.status();
            let resp_headers: Vec<(String, String)> = resp
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();

            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                let code = status.as_u16();

                if let Some(ctx_err) = detect_context_window_error(code, &text, "copilot") {
                    return Err(ctx_err);
                }

                if code == 429 {
                    let (retry_after, _) = parse_rate_limit_headers(&resp_headers, "copilot");
                    return Err(ProviderError::RateLimited {
                        retry_after_secs: retry_after.map(|d| d.as_secs()).unwrap_or(60),
                        retry_after,
                    });
                }
                return Err(ProviderError::Api {
                    status: code,
                    message: text,
                });
            }

            let byte_stream = resp.bytes_stream();
            let stream_state = transformer.new_stream_state(&request.model);
            let sse_parser = SseParser::new();

            let event_stream = stream::unfold(
                (
                    Box::pin(
                        byte_stream.map(|r| r.map_err(|e| ProviderError::Stream(e.to_string()))),
                    ),
                    sse_parser,
                    stream_state,
                    VecDeque::<Result<ChatChunk, ProviderError>>::new(),
                ),
                |(mut inner, mut parser, mut state, mut pending)| async move {
                    loop {
                        if let Some(item) = pending.pop_front() {
                            return Some((item, (inner, parser, state, pending)));
                        }

                        match inner.next().await {
                            Some(Ok(bytes)) => {
                                let text = String::from_utf8_lossy(&bytes);
                                let events = match parser.feed(&text) {
                                    Ok(e) => e,
                                    Err(e) => {
                                        return Some((Err(e), (inner, parser, state, pending)));
                                    }
                                };

                                for event in events {
                                    match event {
                                        SseEvent::Data(data) => match state.process_event(&data) {
                                            Ok(Some(chunk)) => pending.push_back(Ok(chunk)),
                                            Ok(None) => {}
                                            Err(e) => pending.push_back(Err(e)),
                                        },
                                        SseEvent::Done => return None,
                                        SseEvent::Skip => {}
                                    }
                                }
                            }
                            Some(Err(e)) => return Some((Err(e), (inner, parser, state, pending))),
                            None => {
                                if let Ok(Some(event)) = parser.flush() {
                                    if let SseEvent::Data(data) = event {
                                        if let Ok(Some(chunk)) = state.process_event(&data) {
                                            return Some((
                                                Ok(chunk),
                                                (inner, parser, state, pending),
                                            ));
                                        }
                                    }
                                }
                                return None;
                            }
                        }
                    }
                },
            );

            Ok(Box::pin(event_stream)
                as Pin<
                    Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>,
                >)
        })
    }

    fn health_check(&self) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(async move { self.get_token().await.is_ok() })
    }

    fn pricing(&self) -> Vec<ModelPricing> {
        crate::providers::cost::CostCalculator::all()
            .into_iter()
            .filter(|p| p.provider == "copilot")
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct MockTokenStorage {
        token: Option<String>,
    }

    impl MockTokenStorage {
        fn with_token(token: &str) -> Self {
            Self {
                token: Some(token.into()),
            }
        }

        fn empty() -> Self {
            Self { token: None }
        }
    }

    #[async_trait::async_trait]
    impl TokenProvider for MockTokenStorage {
        async fn get_token(&self, _provider: &str) -> Result<String, crate::auth::error::AuthError> {
            self.token
                .clone()
                .ok_or_else(|| crate::auth::error::AuthError::TokenNotFound("copilot".into()))
        }
    }

    #[test]
    fn test_id_and_name() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        assert_eq!(p.id(), "copilot");
        assert_eq!(p.name(), "GitHub Copilot");
    }

    #[test]
    fn test_models_list() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        let models = p.models();
        assert!(models.contains(&"gpt-4o".to_string()));
        assert!(models.contains(&"gpt-4-turbo".to_string()));
        assert!(models.contains(&"o1".to_string()));
        assert!(models.contains(&"o3-mini".to_string()));
    }

    #[test]
    fn test_supports_model() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        assert!(p.supports_model("gpt-4o"));
        assert!(p.supports_model("o1"));
        assert!(!p.supports_model("claude-sonnet-4-20250514"));
    }

    #[tokio::test]
    async fn test_health_check_no_token() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        assert!(!p.health_check().await);
    }

    #[tokio::test]
    async fn test_health_check_with_token() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::with_token("test")));
        assert!(p.health_check().await);
    }

    #[tokio::test]
    async fn test_chat_no_token() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hello".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        };
        let result = p.chat(&req).await;
        assert!(matches!(result, Err(ProviderError::NoToken { .. })));
    }

    #[test]
    fn test_pricing_returns_copilot_models() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        let pricing = p.pricing();
        assert!(!pricing.is_empty());
        for mp in &pricing {
            assert_eq!(mp.provider, "copilot");
        }
    }
}
