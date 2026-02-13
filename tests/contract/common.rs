use gaud::providers::traits::LlmProvider;
use gaud::providers::types::{ChatRequest, ChatMessage, MessageRole, MessageContent};

pub async fn test_provider_chat_basic<P: LlmProvider>(provider: &P, model: &str) {
    let req = ChatRequest {
        model: model.to_string(),
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

    // We expect the provider to handle this request without panicking
    // The actual response depends on the mock backend
    let result = provider.chat(&req).await;
    assert!(result.is_ok(), "Chat request failed: {:?}", result.err());
}

pub async fn test_provider_streaming_basic<P: LlmProvider>(provider: &P, model: &str) {
    let req = ChatRequest {
        model: model.to_string(),
        messages: vec![ChatMessage {
            role: MessageRole::User,
            content: Some(MessageContent::Text("Hello".into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        temperature: None,
        max_tokens: None,
        stream: true,
        top_p: None,
        stop: None,
        tools: None,
        tool_choice: None,
        stream_options: None,
    };

    let result = provider.stream_chat(&req).await;
    assert!(result.is_ok(), "Streaming request failed: {:?}", result.err());
}
