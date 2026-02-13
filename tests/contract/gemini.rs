use gaud::providers::gemini::GeminiProvider;
use gaud::providers::gemini::CloudCodeClient;
use std::sync::Arc;
use gaud::auth::TokenProvider;
use gaud::auth::error::AuthError;
use crate::contract::common;
use wiremock::{MockServer, Mock, ResponseTemplate};
use wiremock::matchers::{method, path};

struct MockTokenProvider;

#[async_trait::async_trait]
impl TokenProvider for MockTokenProvider {
    async fn get_token(&self, _provider: &str) -> Result<String, AuthError> {
        Ok("mock_token".to_string())
    }
}

#[tokio::test]
async fn test_gemini_instantiation() {
    let token_provider = Arc::new(MockTokenProvider);
    let provider = GeminiProvider::new(token_provider);

    assert_eq!(gaud::providers::traits::LlmProvider::id(&provider), "gemini");
}

#[tokio::test]
async fn test_gemini_chat_contract() {
    // 1. Start Mock Server
    let mock_server = MockServer::start().await;

    // 2. Configure Mock Response
    let response_body = serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{
                    "text": "Hello from Gemini"
                }],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": []
        }],
        "usageMetadata": {
            "promptTokenCount": 5,
            "candidatesTokenCount": 5,
            "totalTokenCount": 10
        }
    });

    Mock::given(method("POST"))
        .and(path("/v1internal:generateContent")) // Adjusted to match Gemini default path
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    // 3. Configure Client to use Mock Server
    let token_provider = Arc::new(MockTokenProvider);
    // Explicitly configure the client to point to the mock server
    let client = CloudCodeClient::builder()
        .with_token_provider(token_provider.clone())
        .with_base_url(mock_server.uri())
        .build();

    // Create provider with the configured client
    let provider = GeminiProvider::with_client(client);

    // Run the common contract test
    common::test_provider_chat_basic(&provider, "gemini-1.5-pro").await;
}

#[tokio::test]
async fn test_gemini_streaming_contract() {
    // 1. Start Mock Server
    let mock_server = MockServer::start().await;

    // 2. Configure Mock Response (SSE format)
    // Cloud Code emits SSE with JSON data
    let json_data = serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{ "text": "Streamed content" }]
            },
            "finishReason": "STOP"
        }]
    });
    let sse_body = format!("data: {}\n\n", serde_json::to_string(&json_data).unwrap());

    Mock::given(method("POST"))
        .and(path("/v1internal:streamGenerateContent")) // Streaming endpoint
        .respond_with(ResponseTemplate::new(200)
            .set_body_string(sse_body)
            .insert_header("content-type", "text/event-stream"))
        .mount(&mock_server)
        .await;

    // 3. Configure Client
    let token_provider = Arc::new(MockTokenProvider);
    let client = CloudCodeClient::builder()
        .with_token_provider(token_provider.clone())
        .with_base_url(mock_server.uri())
        .build();

    let provider = GeminiProvider::with_client(client);

    // Run the common streaming contract test
    common::test_provider_streaming_basic(&provider, "gemini-1.5-pro").await;
}
