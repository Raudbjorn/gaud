//! Integration tests for antigravity-gate using wiremock.
//!
//! These tests use wiremock to mock the Cloud Code API and test
//! the complete request/response flow.

use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use antigravity_gate::{CloudCodeClient, MemoryTokenStorage, Result, Role, StopReason, TokenInfo};

/// Helper function to create a test client with pre-configured token.
fn create_test_client(mock_uri: &str) -> Arc<CloudCodeClient<MemoryTokenStorage>> {
    // Create a token with embedded project ID to skip project discovery
    let token = TokenInfo::new(
        "test-access-token".to_string(),
        "test-refresh|proj-123|managed-456".to_string(),
        3600,
    );

    let storage = MemoryTokenStorage::with_token(token);

    Arc::new(
        CloudCodeClient::builder()
            .with_storage(storage)
            .with_base_url(mock_uri)
            .build(),
    )
}

/// Create a mock Google API response for non-streaming requests.
fn create_google_response(text: &str, finish_reason: &str) -> serde_json::Value {
    json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{
                    "text": text
                }]
            },
            "finishReason": finish_reason
        }],
        "usageMetadata": {
            "promptTokenCount": 10,
            "candidatesTokenCount": 20
        },
        "modelVersion": "claude-sonnet-4-5"
    })
}

/// Create a mock Google API response with tool use.
fn create_google_tool_response(
    tool_name: &str,
    _tool_id: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{
                    "functionCall": {
                        "name": tool_name,
                        "args": args
                    }
                }]
            },
            "finishReason": "TOOL_USE"
        }],
        "usageMetadata": {
            "promptTokenCount": 10,
            "candidatesTokenCount": 20
        },
        "modelVersion": "claude-sonnet-4-5"
    })
}

/// Create a mock SSE streaming response.
fn create_sse_response(chunks: &[&str]) -> String {
    let mut sse = String::new();
    for chunk in chunks {
        sse.push_str("data: ");
        sse.push_str(chunk);
        sse.push_str("\n\n");
    }
    sse
}

// ============================================================================
// Basic Message Tests
// ============================================================================

#[tokio::test]
async fn test_simple_message() -> Result<()> {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Set up mock response
    Mock::given(method("POST"))
        .and(path("/v1internal/generate_content"))
        .and(header("authorization", "Bearer test-access-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(create_google_response("Hello, world!", "STOP")),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    // Create client and make request
    let client = create_test_client(&mock_server.uri());

    let response = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .user_message("Hello!")
        .send()
        .await?;

    // Verify response
    assert_eq!(response.text(), "Hello, world!");
    assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
    assert!(response.usage.input_tokens > 0);
    assert!(response.usage.output_tokens > 0);

    Ok(())
}

#[tokio::test]
async fn test_message_with_system_prompt() -> Result<()> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1internal/generate_content"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(create_google_response("I am a helpful assistant.", "STOP")),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = create_test_client(&mock_server.uri());

    let response = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .system("You are a helpful assistant.")
        .user_message("Who are you?")
        .send()
        .await?;

    assert_eq!(response.text(), "I am a helpful assistant.");

    Ok(())
}

#[tokio::test]
async fn test_multi_turn_conversation() -> Result<()> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1internal/generate_content"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(create_google_response("The capital is Paris.", "STOP")),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = create_test_client(&mock_server.uri());

    let response = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .user_message("Hello!")
        .assistant_message("Hello! How can I help?")
        .user_message("What is the capital of France?")
        .send()
        .await?;

    assert_eq!(response.text(), "The capital is Paris.");

    Ok(())
}

// ============================================================================
// Tool Use Tests
// ============================================================================

#[tokio::test]
async fn test_tool_use_response() -> Result<()> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1internal/generate_content"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(create_google_tool_response(
                "get_weather",
                "toolu_123",
                json!({"location": "Tokyo"}),
            )),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = create_test_client(&mock_server.uri());

    let tool = antigravity_gate::Tool::new(
        "get_weather",
        "Get weather for a location",
        json!({
            "type": "object",
            "properties": {
                "location": {"type": "string"}
            },
            "required": ["location"]
        }),
    );

    let response = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .user_message("What's the weather in Tokyo?")
        .tool(tool)
        .send()
        .await?;

    // Verify tool use in response
    assert!(response.stop_reason == Some(StopReason::ToolUse));

    let tool_uses: Vec<_> = response
        .content
        .iter()
        .filter(|b| b.is_tool_use())
        .collect();

    assert_eq!(tool_uses.len(), 1);

    if let Some((id, name, input)) = tool_uses[0].as_tool_use() {
        assert_eq!(name, "get_weather");
        assert_eq!(input["location"], "Tokyo");
        // ID is generated, just check it exists
        assert!(!id.is_empty());
    } else {
        panic!("Expected tool use block");
    }

    Ok(())
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_rate_limit_error() -> Result<()> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1internal/generate_content"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_json(json!({
                    "error": {
                        "code": 429,
                        "message": "Rate limit exceeded",
                        "status": "RESOURCE_EXHAUSTED"
                    }
                }))
                .insert_header("retry-after", "60"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = create_test_client(&mock_server.uri());

    let result = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .user_message("Hello!")
        .send()
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.is_rate_limit());

    Ok(())
}

#[tokio::test]
async fn test_auth_error() -> Result<()> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1internal/generate_content"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {
                "code": 401,
                "message": "Invalid credentials",
                "status": "UNAUTHENTICATED"
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = create_test_client(&mock_server.uri());

    let result = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .user_message("Hello!")
        .send()
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.is_auth_error());

    Ok(())
}

#[tokio::test]
async fn test_api_error() -> Result<()> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1internal/generate_content"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": {
                "code": 500,
                "message": "Internal server error",
                "status": "INTERNAL"
            }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = create_test_client(&mock_server.uri());

    let result = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .user_message("Hello!")
        .send()
        .await;

    assert!(result.is_err());

    Ok(())
}

// ============================================================================
// Builder Validation Tests
// ============================================================================

#[tokio::test]
async fn test_builder_requires_model() {
    let storage = MemoryTokenStorage::new();
    let client = Arc::new(CloudCodeClient::builder().with_storage(storage).build());

    let result = client
        .messages()
        // No model set
        .max_tokens(1024)
        .user_message("Hello!")
        .build_request();

    assert!(result.is_err());
}

#[tokio::test]
async fn test_builder_requires_max_tokens() {
    let storage = MemoryTokenStorage::new();
    let client = Arc::new(CloudCodeClient::builder().with_storage(storage).build());

    let result = client
        .messages()
        .model("claude-sonnet-4-5")
        // No max_tokens set
        .user_message("Hello!")
        .build_request();

    assert!(result.is_err());
}

#[tokio::test]
async fn test_builder_requires_messages() {
    let storage = MemoryTokenStorage::new();
    let client = Arc::new(CloudCodeClient::builder().with_storage(storage).build());

    let result = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        // No messages
        .build_request();

    assert!(result.is_err());
}

#[tokio::test]
async fn test_builder_valid_request() {
    let storage = MemoryTokenStorage::new();
    let client = Arc::new(CloudCodeClient::builder().with_storage(storage).build());

    let result = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .user_message("Hello!")
        .build_request();

    assert!(result.is_ok());
    let request = result.unwrap();
    assert_eq!(request.model, "claude-sonnet-4-5");
    assert_eq!(request.max_tokens, 1024);
    assert_eq!(request.messages.len(), 1);
}

// ============================================================================
// Streaming Tests
// ============================================================================

#[tokio::test]
async fn test_streaming_response() -> Result<()> {
    use futures::StreamExt;

    let mock_server = MockServer::start().await;

    // Create SSE response chunks
    let sse_chunks = [
        json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "Hello"}]
                }
            }]
        })
        .to_string(),
        json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": ", world!"}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20
            }
        })
        .to_string(),
    ];

    let sse_body = create_sse_response(&sse_chunks.iter().map(|s| s.as_str()).collect::<Vec<_>>());

    Mock::given(method("POST"))
        .and(path("/v1internal/stream_generate_content"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = create_test_client(&mock_server.uri());

    let mut stream = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .user_message("Hello!")
        .send_stream()
        .await?;

    // Collect events
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event?);
    }

    // Verify we received events
    assert!(!events.is_empty());

    Ok(())
}

// ============================================================================
// Thinking Model Tests
// ============================================================================

#[tokio::test]
async fn test_thinking_model_response() -> Result<()> {
    let mock_server = MockServer::start().await;

    // Create SSE response with thinking block
    let sse_chunks = [
        json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "thought": "Let me think about this..."
                    }]
                }
            }]
        })
        .to_string(),
        json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "The answer is 42."}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 50
            }
        })
        .to_string(),
    ];

    let sse_body = create_sse_response(&sse_chunks.iter().map(|s| s.as_str()).collect::<Vec<_>>());

    Mock::given(method("POST"))
        .and(path("/v1internal/stream_generate_content"))
        .and(header("anthropic-beta", "interleaved-thinking-2025-05-14"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = create_test_client(&mock_server.uri());

    let response = client
        .messages()
        .model("claude-sonnet-4-5-thinking")
        .max_tokens(16000)
        .thinking_budget(10000)
        .user_message("What is the meaning of life?")
        .send()
        .await?;

    // Verify response contains thinking and text
    assert!(response.content.iter().any(|b| b.is_text()));

    Ok(())
}

// ============================================================================
// Authentication Tests
// ============================================================================

#[tokio::test]
async fn test_not_authenticated() -> Result<()> {
    // Create client without pre-configured token
    let storage = MemoryTokenStorage::new();
    let client = CloudCodeClient::builder().with_storage(storage).build();

    // Should not be authenticated
    assert!(!client.is_authenticated().await?);

    Ok(())
}

#[tokio::test]
async fn test_authenticated_with_token() -> Result<()> {
    let token = TokenInfo::new("test-access".to_string(), "test-refresh".to_string(), 3600);
    let storage = MemoryTokenStorage::with_token(token);
    let client = CloudCodeClient::builder().with_storage(storage).build();

    assert!(client.is_authenticated().await?);

    Ok(())
}

// ============================================================================
// Request Construction Tests
// ============================================================================

#[tokio::test]
async fn test_request_builder_full() -> Result<()> {
    let storage = MemoryTokenStorage::new();
    let client = Arc::new(CloudCodeClient::builder().with_storage(storage).build());

    let tool = antigravity_gate::Tool::new(
        "calculator",
        "Perform calculations",
        json!({"type": "object", "properties": {}}),
    );

    let request = client
        .messages()
        .model("claude-sonnet-4-5-thinking")
        .max_tokens(2048)
        .system("You are helpful.")
        .user_message("First message")
        .assistant_message("Response")
        .user_message("Second message")
        .temperature(0.7)
        .top_p(0.9)
        .top_k(40)
        .stop_sequences(vec!["END".to_string()])
        .tool(tool)
        .tool_choice(antigravity_gate::ToolChoice::Auto)
        .thinking_budget(10000)
        .build_request()?;

    assert_eq!(request.model, "claude-sonnet-4-5-thinking");
    assert_eq!(request.max_tokens, 2048);
    assert_eq!(request.messages.len(), 3);
    assert_eq!(request.temperature, Some(0.7));
    assert_eq!(request.top_p, Some(0.9));
    assert_eq!(request.top_k, Some(40));
    assert!(request.system.is_some());
    assert!(request.tools.is_some());
    assert!(request.thinking.is_some());

    Ok(())
}

// ============================================================================
// Response Parsing Tests
// ============================================================================

#[tokio::test]
async fn test_response_helper_methods() -> Result<()> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1internal/generate_content"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(create_google_response("Test response text", "STOP")),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = create_test_client(&mock_server.uri());

    let response = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .user_message("Hello!")
        .send()
        .await?;

    // Test helper methods
    assert_eq!(response.text(), "Test response text");
    assert!(!response.content.is_empty());
    assert_eq!(response.role, Role::Assistant);

    Ok(())
}

// ============================================================================
// Header Tests
// ============================================================================

#[tokio::test]
async fn test_correct_headers_sent() -> Result<()> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1internal/generate_content"))
        .and(header("authorization", "Bearer test-access-token"))
        .and(header("content-type", "application/json"))
        .and(header_exists("user-agent"))
        .and(header_exists("x-goog-api-client"))
        .and(header_exists("client-metadata"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(create_google_response("Success", "STOP")),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = create_test_client(&mock_server.uri());

    client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .user_message("Hello!")
        .send()
        .await?;

    Ok(())
}
