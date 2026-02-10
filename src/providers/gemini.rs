//! Gemini (Google) Provider
//!
//! Converts OpenAI-format requests into Google Generative AI content format,
//! sends them to the Gemini API, and converts the response back to OpenAI
//! format.

use std::collections::VecDeque;
use std::pin::Pin;

use futures::stream::{self, StreamExt};
use futures::Stream;
use reqwest::Client;

use crate::providers::transform::{GeminiTransformer, SseEvent, SseParser};
use crate::providers::transform::util::{detect_context_window_error, parse_rate_limit_headers};
use crate::providers::transformer::{ProviderResponseMeta, ProviderTransformer};
use crate::providers::types::*;
use crate::providers::pricing::ModelPricing;
use crate::providers::{LlmProvider, ProviderError, TokenStorage};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

const SUPPORTED_MODELS: &[&str] = &[
    "gemini-2.5-flash",
    "gemini-2.5-pro",
    "gemini-2.0-flash",
];

// ---------------------------------------------------------------------------
// Gemini Provider
// ---------------------------------------------------------------------------

/// LLM provider that communicates with the Google Gemini API.
pub struct GeminiProvider<T: TokenStorage> {
    http: Client,
    tokens: std::sync::Arc<T>,
}

impl<T: TokenStorage + 'static> GeminiProvider<T> {
    /// Create a new Gemini provider backed by the given token storage.
    pub fn new(tokens: std::sync::Arc<T>) -> Self {
        Self {
            http: Client::new(),
            tokens,
        }
    }

    /// Retrieve an access token or return an error.
    async fn get_token(&self) -> Result<String, ProviderError> {
        self.tokens
            .get_access_token("gemini")
            .await?
            .ok_or_else(|| ProviderError::NoToken {
                provider: "gemini".to_string(),
            })
    }
}

// ---------------------------------------------------------------------------
// LlmProvider implementation
// ---------------------------------------------------------------------------

impl<T: TokenStorage + 'static> LlmProvider for GeminiProvider<T> {
    fn id(&self) -> &str {
        "gemini"
    }

    fn name(&self) -> &str {
        "Gemini (Google)"
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
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        let request = request.clone();
        Box::pin(async move {
            if !self.supports_model(&request.model) {
                return Err(ProviderError::Other(format!(
                    "Unsupported Gemini model: {}",
                    request.model
                )));
            }

            let token = self.get_token().await?;
            let transformer = GeminiTransformer::new();
            let body = transformer.transform_request(&request)?;

            let url = format!("{}/{}:generateContent", API_BASE, request.model);

            let resp = self
                .http
                .post(&url)
                .bearer_auth(&token)
                .header("content-type", "application/json")
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

                if let Some(ctx_err) = detect_context_window_error(code, &text, "gemini") {
                    return Err(ctx_err);
                }

                if code == 429 {
                    let (retry_after, _) = parse_rate_limit_headers(&resp_headers, "gemini");
                    return Err(ProviderError::RateLimited {
                        retry_after_secs: retry_after
                            .map(|d| d.as_secs())
                            .unwrap_or(60),
                        retry_after,
                    });
                }
                return Err(ProviderError::Api {
                    status: code,
                    message: text,
                });
            }

            let (_, rate_limit_headers) = parse_rate_limit_headers(&resp_headers, "gemini");
            let response_json: serde_json::Value = resp.json().await?;
            let meta = ProviderResponseMeta {
                provider: "gemini".into(),
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
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>, ProviderError>> + Send + '_>> {
        let request = request.clone();
        Box::pin(async move {
            if !self.supports_model(&request.model) {
                return Err(ProviderError::Other(format!(
                    "Unsupported Gemini model: {}",
                    request.model
                )));
            }

            let token = self.get_token().await?;
            let transformer = GeminiTransformer::new();
            let body = transformer.transform_request(&request)?;

            let url = format!(
                "{}/{}:streamGenerateContent?alt=sse",
                API_BASE, request.model
            );

            let resp = self
                .http
                .post(&url)
                .bearer_auth(&token)
                .header("content-type", "application/json")
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

                if let Some(ctx_err) = detect_context_window_error(code, &text, "gemini") {
                    return Err(ctx_err);
                }

                if code == 429 {
                    let (retry_after, _) = parse_rate_limit_headers(&resp_headers, "gemini");
                    return Err(ProviderError::RateLimited {
                        retry_after_secs: retry_after
                            .map(|d| d.as_secs())
                            .unwrap_or(60),
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
                    Box::pin(byte_stream.map(|r| r.map_err(|e| ProviderError::Stream(e.to_string())))),
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
                                    Err(e) => return Some((Err(e), (inner, parser, state, pending))),
                                };

                                for event in events {
                                    match event {
                                        SseEvent::Data(data) => {
                                            match state.process_event(&data) {
                                                Ok(Some(chunk)) => pending.push_back(Ok(chunk)),
                                                Ok(None) => {}
                                                Err(e) => pending.push_back(Err(e)),
                                            }
                                        }
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
                                            return Some((Ok(chunk), (inner, parser, state, pending)));
                                        }
                                    }
                                }
                                return None;
                            }
                        }
                    }
                },
            );

            Ok(Box::pin(event_stream) as Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>)
        })
    }

    fn health_check(&self) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(async move { self.get_token().await.is_ok() })
    }

    fn pricing(&self) -> Vec<ModelPricing> {
        crate::providers::cost::CostCalculator::all()
            .into_iter()
            .filter(|p| p.provider == "gemini")
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

    impl TokenStorage for MockTokenStorage {
        async fn get_access_token(
            &self,
            _provider: &str,
        ) -> Result<Option<String>, ProviderError> {
            Ok(self.token.clone())
        }
    }

    #[test]
    fn test_id_and_name() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        assert_eq!(p.id(), "gemini");
        assert_eq!(p.name(), "Gemini (Google)");
    }

    #[test]
    fn test_models_list() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        let models = p.models();
        assert!(models.contains(&"gemini-2.5-flash".to_string()));
        assert!(models.contains(&"gemini-2.5-pro".to_string()));
        assert!(models.contains(&"gemini-2.0-flash".to_string()));
    }

    #[test]
    fn test_supports_model() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        assert!(p.supports_model("gemini-2.5-flash"));
        assert!(!p.supports_model("gpt-4o"));
    }

    #[tokio::test]
    async fn test_health_check_no_token() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        assert!(!p.health_check().await);
    }

    #[tokio::test]
    async fn test_health_check_with_token() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::with_token("test")));
        assert!(p.health_check().await);
    }

    #[tokio::test]
    async fn test_chat_no_token() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        let req = ChatRequest {
            model: "gemini-2.5-flash".into(),
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

    #[tokio::test]
    async fn test_chat_rejects_unsupported_model() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::with_token("test")));
        let req = ChatRequest {
            model: "../../admin".into(),
            messages: vec![],
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
        assert!(matches!(result, Err(ProviderError::Other(_))));
    }

    #[tokio::test]
    async fn test_stream_rejects_unsupported_model() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::with_token("test")));
        let req = ChatRequest {
            model: "gemini-evil/../hack".into(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: true,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        };
        let result = p.stream_chat(&req).await;
        assert!(matches!(result, Err(ProviderError::Other(_))));
    }

    #[test]
    fn test_pricing_returns_gemini_models() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        let pricing = p.pricing();
        assert!(!pricing.is_empty());
        for mp in &pricing {
            assert_eq!(mp.provider, "gemini");
        }
    }
}
