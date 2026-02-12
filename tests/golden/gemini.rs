use gaud::providers::gemini::GeminiProvider;
use gaud::providers::types::{ChatRequest, ChatResponse};
use gaud::providers::gemini::models::MessagesResponse;
use std::sync::Arc;
use crate::golden::GoldenTest;

struct MockTokenProvider;
#[async_trait::async_trait]
impl gaud::auth::TokenProvider for MockTokenProvider {
    async fn get_token(&self, _: &str) -> Result<String, gaud::auth::error::AuthError> {
        Ok("mock".into())
    }
}

#[test]
fn test_gemini_request_transformation() {
    let golden = GoldenTest::new("gemini");
    let provider = GeminiProvider::new(Arc::new(MockTokenProvider));

    // Load generic ChatRequest
    let req: ChatRequest = golden.load_json("simple_chat_req");

    // Transform
    let google_req = provider.convert_request(&req).expect("Failed to convert request");

    // Assert matches expected Google format
    // Note: We might need to filter out some fields or ensure strict matching
    golden.assert_json("simple_chat_req_google", &google_req);
}

#[test]
fn test_gemini_response_transformation() {
    let golden = GoldenTest::new("gemini");
    let provider = GeminiProvider::new(Arc::new(MockTokenProvider));

    // Load generic Google response
    // We need to deserialize it into MessagesResponse first
    let google_resp: MessagesResponse = golden.load_json("simple_chat_resp");

    // Transform
    let mut chat_resp = provider.convert_response(google_resp, "gemini-1.5-pro").expect("Failed to convert response");

    // Fix dynamic fields for stable comparison
    chat_resp.created = 1234567890;
    chat_resp.id = "msg_mock".to_string();

    // Assert matches expected ChatResponse
    golden.assert_json("simple_chat_resp_internal", &chat_resp);
}
