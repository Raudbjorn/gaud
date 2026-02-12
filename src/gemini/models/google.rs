//! Google Generative AI API types (internal).
//!
//! This module contains types for the Google Generative AI API format.
//! These types are used internally for request/response conversion
//! and are not part of the public API.

// Allow dead code since these types will be used by the convert module in Phase 5
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Request format for Google Generative AI API.
///
/// This is the wrapper format required by Cloud Code.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleRequest {
    /// The conversation contents.
    pub contents: Vec<Content>,

    /// Generation configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,

    /// System instruction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,

    /// Tool definitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GoogleTool>>,

    /// Tool configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<ToolConfig>,

    /// Thinking configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<GoogleThinkingConfig>,

    /// Session ID for caching continuity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl GoogleRequest {
    /// Create a new empty request.
    pub fn new() -> Self {
        Self {
            contents: Vec::new(),
            generation_config: None,
            system_instruction: None,
            tools: None,
            tool_config: None,
            thinking_config: None,
            session_id: None,
        }
    }

    /// Create a request with contents.
    pub fn with_contents(contents: Vec<Content>) -> Self {
        Self {
            contents,
            generation_config: None,
            system_instruction: None,
            tools: None,
            tool_config: None,
            thinking_config: None,
            session_id: None,
        }
    }
}

impl Default for GoogleRequest {
    fn default() -> Self {
        Self::new()
    }
}

/// Response from Google Generative AI API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleResponse {
    /// Generated candidates.
    #[serde(default)]
    pub candidates: Vec<Candidate>,

    /// Usage metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_metadata: Option<UsageMetadata>,

    /// Model version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,
}

impl GoogleResponse {
    /// Get the first candidate if available.
    pub fn first_candidate(&self) -> Option<&Candidate> {
        self.candidates.first()
    }

    /// Get the content from the first candidate.
    pub fn content(&self) -> Option<&Content> {
        self.first_candidate().and_then(|c| c.content.as_ref())
    }
}

/// A generated candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Candidate {
    /// Generated content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Content>,

    /// Finish reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,

    /// Safety ratings.
    #[serde(default)]
    pub safety_ratings: Vec<SafetyRating>,

    /// Citation metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citation_metadata: Option<Value>,

    /// Candidate index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
}

/// Safety rating for content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafetyRating {
    /// Category of the rating.
    pub category: String,

    /// Probability level.
    pub probability: String,
}

/// Content in a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Content {
    /// The role ("user" or "model").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// Parts of the content.
    pub parts: Vec<Part>,
}

impl Content {
    /// Create a user content.
    pub fn user(parts: Vec<Part>) -> Self {
        Self {
            role: Some("user".to_string()),
            parts,
        }
    }

    /// Create a model content.
    pub fn model(parts: Vec<Part>) -> Self {
        Self {
            role: Some("model".to_string()),
            parts,
        }
    }

    /// Create a system instruction (no role).
    pub fn system(parts: Vec<Part>) -> Self {
        Self { role: None, parts }
    }

    /// Create a content with a single text part.
    pub fn text(role: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            role: Some(role.into()),
            parts: vec![Part::text(text)],
        }
    }
}

/// A part of content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Part {
    /// Text content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Function call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,

    /// Function response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_response: Option<FunctionResponse>,

    /// Inline data (for images, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<InlineData>,

    /// Thought content (for thinking models).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought: Option<bool>,

    /// Thought signature (for Gemini thinking continuity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,

    /// File data reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_data: Option<FileData>,
}

impl Part {
    /// Create a text part.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            function_call: None,
            function_response: None,
            inline_data: None,
            thought: None,
            thought_signature: None,
            file_data: None,
        }
    }

    /// Create a thought/thinking part.
    pub fn thought(text: impl Into<String>, signature: Option<String>) -> Self {
        Self {
            text: Some(text.into()),
            function_call: None,
            function_response: None,
            inline_data: None,
            thought: Some(true),
            thought_signature: signature,
            file_data: None,
        }
    }

    /// Create a function call part.
    pub fn function_call(call: FunctionCall) -> Self {
        Self {
            text: None,
            function_call: Some(call),
            function_response: None,
            inline_data: None,
            thought: None,
            thought_signature: None,
            file_data: None,
        }
    }

    /// Create a function response part.
    pub fn function_response(response: FunctionResponse) -> Self {
        Self {
            text: None,
            function_call: None,
            function_response: Some(response),
            inline_data: None,
            thought: None,
            thought_signature: None,
            file_data: None,
        }
    }

    /// Create an inline data part.
    pub fn inline_data(data: InlineData) -> Self {
        Self {
            text: None,
            function_call: None,
            function_response: None,
            inline_data: Some(data),
            thought: None,
            thought_signature: None,
            file_data: None,
        }
    }

    /// Check if this is a text part.
    pub fn is_text(&self) -> bool {
        self.text.is_some() && self.thought.is_none()
    }

    /// Check if this is a thought/thinking part.
    pub fn is_thought(&self) -> bool {
        self.thought.unwrap_or(false)
    }

    /// Check if this is a function call part.
    pub fn is_function_call(&self) -> bool {
        self.function_call.is_some()
    }

    /// Check if this is a function response part.
    pub fn is_function_response(&self) -> bool {
        self.function_response.is_some()
    }
}

/// A function call from the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FunctionCall {
    /// Name of the function to call.
    pub name: String,

    /// Arguments as JSON.
    pub args: Value,

    /// Unique ID for this function call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl FunctionCall {
    /// Create a new function call.
    pub fn new(name: impl Into<String>, args: Value) -> Self {
        Self {
            name: name.into(),
            args,
            id: None,
        }
    }

    /// Create a function call with an ID.
    pub fn with_id(name: impl Into<String>, args: Value, id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args,
            id: Some(id.into()),
        }
    }
}

/// A function response to send back to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FunctionResponse {
    /// Name of the function that was called.
    pub name: String,

    /// Response data.
    pub response: FunctionResponseData,

    /// ID of the function call being responded to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl FunctionResponse {
    /// Create a new function response.
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            response: FunctionResponseData {
                content: content.into(),
            },
            id: None,
        }
    }

    /// Create a function response with an ID.
    pub fn with_id(
        name: impl Into<String>,
        content: impl Into<String>,
        id: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            response: FunctionResponseData {
                content: content.into(),
            },
            id: Some(id.into()),
        }
    }
}

/// Data in a function response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FunctionResponseData {
    /// Text content of the response.
    pub content: String,
}

/// Inline data (for images, audio, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InlineData {
    /// MIME type of the data.
    pub mime_type: String,

    /// Base64-encoded data.
    pub data: String,
}

impl InlineData {
    /// Create new inline data.
    pub fn new(mime_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            mime_type: mime_type.into(),
            data: data.into(),
        }
    }
}

/// File data reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileData {
    /// MIME type of the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,

    /// URI of the file.
    pub file_uri: String,
}

/// Generation configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerationConfig {
    /// Maximum output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,

    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Top-p sampling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// Top-k sampling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,

    /// Stop sequences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,

    /// Candidate count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_count: Option<u32>,

    /// Response MIME type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_mime_type: Option<String>,
}

impl GenerationConfig {
    /// Create a new generation config with max tokens.
    pub fn new(max_output_tokens: u32) -> Self {
        Self {
            max_output_tokens: Some(max_output_tokens),
            ..Default::default()
        }
    }
}

/// Tool definition for Google API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleTool {
    /// Function declarations.
    pub function_declarations: Vec<FunctionDeclaration>,
}

impl GoogleTool {
    /// Create a tool with function declarations.
    pub fn new(declarations: Vec<FunctionDeclaration>) -> Self {
        Self {
            function_declarations: declarations,
        }
    }
}

/// Function declaration for tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FunctionDeclaration {
    /// Function name.
    pub name: String,

    /// Function description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Parameter schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

impl FunctionDeclaration {
    /// Create a new function declaration.
    pub fn new(
        name: impl Into<String>,
        description: Option<String>,
        parameters: Option<Value>,
    ) -> Self {
        Self {
            name: name.into(),
            description,
            parameters,
        }
    }
}

/// Tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolConfig {
    /// Function calling config.
    pub function_calling_config: FunctionCallingConfig,
}

impl ToolConfig {
    /// Create an AUTO tool config.
    pub fn auto() -> Self {
        Self {
            function_calling_config: FunctionCallingConfig {
                mode: "AUTO".to_string(),
                allowed_function_names: None,
            },
        }
    }

    /// Create an ANY tool config.
    pub fn any() -> Self {
        Self {
            function_calling_config: FunctionCallingConfig {
                mode: "ANY".to_string(),
                allowed_function_names: None,
            },
        }
    }

    /// Create a NONE tool config.
    pub fn none() -> Self {
        Self {
            function_calling_config: FunctionCallingConfig {
                mode: "NONE".to_string(),
                allowed_function_names: None,
            },
        }
    }

    /// Create a tool config that forces a specific function.
    pub fn force(function_name: impl Into<String>) -> Self {
        Self {
            function_calling_config: FunctionCallingConfig {
                mode: "ANY".to_string(),
                allowed_function_names: Some(vec![function_name.into()]),
            },
        }
    }
}

/// Function calling configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FunctionCallingConfig {
    /// Mode: AUTO, ANY, or NONE.
    pub mode: String,

    /// Allowed function names (for forcing specific functions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_function_names: Option<Vec<String>>,
}

/// Thinking configuration for Google API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleThinkingConfig {
    /// Include thoughts in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,

    /// Thinking budget (for Gemini).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,

    /// Budget tokens (for Claude).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}

impl GoogleThinkingConfig {
    /// Create a Gemini thinking config.
    pub fn gemini(thinking_budget: u32) -> Self {
        Self {
            include_thoughts: Some(true),
            thinking_budget: Some(thinking_budget),
            budget_tokens: None,
        }
    }

    /// Create a Claude thinking config.
    pub fn claude(budget_tokens: u32) -> Self {
        Self {
            include_thoughts: None,
            thinking_budget: None,
            budget_tokens: Some(budget_tokens),
        }
    }
}

/// Usage metadata from Google API.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageMetadata {
    /// Prompt token count.
    #[serde(default)]
    pub prompt_token_count: u32,

    /// Candidates token count.
    #[serde(default)]
    pub candidates_token_count: u32,

    /// Total token count.
    #[serde(default)]
    pub total_token_count: u32,

    /// Cached content token count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_content_token_count: Option<u32>,

    /// Thoughts token count (for thinking models).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thoughts_token_count: Option<u32>,
}

impl UsageMetadata {
    /// Calculate effective input tokens (excluding cache).
    pub fn effective_input_tokens(&self) -> u32 {
        self.prompt_token_count - self.cached_content_token_count.unwrap_or(0)
    }
}

/// Wrapper for Cloud Code API requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloudCodeWrapper {
    /// Project ID.
    pub project: String,

    /// Model name.
    pub model: String,

    /// The actual request.
    pub request: GoogleRequest,

    /// User agent identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,

    /// Request type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_type: Option<String>,

    /// Request ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl CloudCodeWrapper {
    /// Create a new Cloud Code wrapper.
    pub fn new(
        project: impl Into<String>,
        model: impl Into<String>,
        request: GoogleRequest,
    ) -> Self {
        Self {
            project: project.into(),
            model: model.into(),
            request,
            user_agent: Some("antigravity".to_string()),
            request_type: Some("agent".to_string()),
            request_id: None,
        }
    }

    /// Set the request ID.
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_google_request_creation() {
        let request = GoogleRequest::new();
        assert!(request.contents.is_empty());
        assert!(request.generation_config.is_none());
    }

    #[test]
    fn test_google_request_with_contents() {
        let request = GoogleRequest::with_contents(vec![
            Content::user(vec![Part::text("Hello")]),
            Content::model(vec![Part::text("Hi there!")]),
        ]);

        assert_eq!(request.contents.len(), 2);
    }

    #[test]
    fn test_google_request_serialization() {
        let mut request = GoogleRequest::new();
        request.contents = vec![Content::user(vec![Part::text("Test")])];
        request.generation_config = Some(GenerationConfig::new(1024));

        let json = serde_json::to_value(&request).unwrap();
        assert!(json["contents"].is_array());
        assert_eq!(json["generationConfig"]["maxOutputTokens"], 1024);
    }

    #[test]
    fn test_content_creation() {
        let user = Content::user(vec![Part::text("Hello")]);
        assert_eq!(user.role, Some("user".to_string()));

        let model = Content::model(vec![Part::text("Hi")]);
        assert_eq!(model.role, Some("model".to_string()));

        let system = Content::system(vec![Part::text("Be helpful")]);
        assert!(system.role.is_none());
    }

    #[test]
    fn test_part_text() {
        let part = Part::text("Hello, world!");
        assert!(part.is_text());
        assert!(!part.is_thought());
        assert!(!part.is_function_call());

        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["text"], "Hello, world!");
    }

    #[test]
    fn test_part_thought() {
        let part = Part::thought("Let me think...", Some("sig123".to_string()));
        assert!(part.is_thought());
        assert!(!part.is_text());

        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["text"], "Let me think...");
        assert_eq!(json["thought"], true);
        assert_eq!(json["thoughtSignature"], "sig123");
    }

    #[test]
    fn test_part_function_call() {
        let call = FunctionCall::with_id("get_weather", json!({"location": "NYC"}), "call_123");
        let part = Part::function_call(call);

        assert!(part.is_function_call());

        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["functionCall"]["name"], "get_weather");
        assert_eq!(json["functionCall"]["id"], "call_123");
    }

    #[test]
    fn test_part_function_response() {
        let response = FunctionResponse::with_id("get_weather", "Sunny, 72F", "call_123");
        let part = Part::function_response(response);

        assert!(part.is_function_response());

        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["functionResponse"]["name"], "get_weather");
        assert_eq!(
            json["functionResponse"]["response"]["content"],
            "Sunny, 72F"
        );
    }

    #[test]
    fn test_part_inline_data() {
        let data = InlineData::new("image/png", "iVBORw0KGgo=");
        let part = Part::inline_data(data);

        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["inlineData"]["mimeType"], "image/png");
        assert_eq!(json["inlineData"]["data"], "iVBORw0KGgo=");
    }

    #[test]
    fn test_function_declaration() {
        let decl = FunctionDeclaration::new(
            "search",
            Some("Search the web".to_string()),
            Some(json!({"type": "object", "properties": {"query": {"type": "string"}}})),
        );

        let json = serde_json::to_value(&decl).unwrap();
        assert_eq!(json["name"], "search");
        assert_eq!(json["description"], "Search the web");
    }

    #[test]
    fn test_google_tool() {
        let tool = GoogleTool::new(vec![FunctionDeclaration::new("test", None, None)]);

        let json = serde_json::to_value(&tool).unwrap();
        assert!(json["functionDeclarations"].is_array());
        assert_eq!(json["functionDeclarations"][0]["name"], "test");
    }

    #[test]
    fn test_tool_config_modes() {
        let auto = ToolConfig::auto();
        let json = serde_json::to_value(&auto).unwrap();
        assert_eq!(json["functionCallingConfig"]["mode"], "AUTO");

        let any = ToolConfig::any();
        let json = serde_json::to_value(&any).unwrap();
        assert_eq!(json["functionCallingConfig"]["mode"], "ANY");

        let none = ToolConfig::none();
        let json = serde_json::to_value(&none).unwrap();
        assert_eq!(json["functionCallingConfig"]["mode"], "NONE");

        let force = ToolConfig::force("specific_function");
        let json = serde_json::to_value(&force).unwrap();
        assert_eq!(json["functionCallingConfig"]["mode"], "ANY");
        assert!(json["functionCallingConfig"]["allowedFunctionNames"]
            .as_array()
            .unwrap()
            .contains(&json!("specific_function")));
    }

    #[test]
    fn test_thinking_config_gemini() {
        let config = GoogleThinkingConfig::gemini(8000);

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["includeThoughts"], true);
        assert_eq!(json["thinkingBudget"], 8000);
        assert!(json.get("budgetTokens").is_none());
    }

    #[test]
    fn test_thinking_config_claude() {
        let config = GoogleThinkingConfig::claude(10000);

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["budgetTokens"], 10000);
        assert!(json.get("includeThoughts").is_none());
    }

    #[test]
    fn test_generation_config() {
        let config = GenerationConfig {
            max_output_tokens: Some(2048),
            temperature: Some(0.7),
            top_p: Some(0.9),
            top_k: Some(40),
            stop_sequences: Some(vec!["END".to_string()]),
            candidate_count: None,
            response_mime_type: None,
        };

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["maxOutputTokens"], 2048);
        // Use approximate comparison for f32 to f64 conversion
        let temp = json["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 0.0001);
        let top_p = json["topP"].as_f64().unwrap();
        assert!((top_p - 0.9).abs() < 0.0001);
        assert_eq!(json["topK"], 40);
    }

    #[test]
    fn test_usage_metadata() {
        let usage = UsageMetadata {
            prompt_token_count: 100,
            candidates_token_count: 50,
            total_token_count: 150,
            cached_content_token_count: Some(20),
            thoughts_token_count: Some(30),
        };

        assert_eq!(usage.effective_input_tokens(), 80);
    }

    #[test]
    fn test_google_response_deserialization() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "Hello!"}]
                },
                "finishReason": "STOP",
                "safetyRatings": []
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "totalTokenCount": 15
            }
        }"#;

        let response: GoogleResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.candidates.len(), 1);
        assert!(response.first_candidate().is_some());
        assert!(response.content().is_some());
    }

    #[test]
    fn test_cloud_code_wrapper() {
        let request = GoogleRequest::with_contents(vec![Content::user(vec![Part::text("Hello")])]);
        let wrapper = CloudCodeWrapper::new("project-123", "claude-sonnet-4-5", request)
            .with_request_id("req-456");

        let json = serde_json::to_value(&wrapper).unwrap();
        assert_eq!(json["project"], "project-123");
        assert_eq!(json["model"], "claude-sonnet-4-5");
        assert_eq!(json["userAgent"], "antigravity");
        assert_eq!(json["requestType"], "agent");
        assert_eq!(json["requestId"], "req-456");
    }

    #[test]
    fn test_candidate_content() {
        let candidate = Candidate {
            content: Some(Content::model(vec![Part::text("Response")])),
            finish_reason: Some("STOP".to_string()),
            safety_ratings: vec![],
            citation_metadata: None,
            index: Some(0),
        };

        assert!(candidate.content.is_some());
        assert_eq!(candidate.finish_reason, Some("STOP".to_string()));
    }

    #[test]
    fn test_request_roundtrip() {
        let original = GoogleRequest {
            contents: vec![
                Content::user(vec![Part::text("Hello")]),
                Content::model(vec![Part::text("Hi!")]),
            ],
            generation_config: Some(GenerationConfig::new(1024)),
            system_instruction: Some(Content::system(vec![Part::text("Be helpful")])),
            tools: Some(vec![GoogleTool::new(vec![FunctionDeclaration::new(
                "test",
                Some("A test function".to_string()),
                Some(json!({"type": "object"})),
            )])]),
            tool_config: Some(ToolConfig::auto()),
            thinking_config: Some(GoogleThinkingConfig::gemini(8000)),
            session_id: None,
        };

        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: GoogleRequest = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.contents.len(), 2);
        assert!(deserialized.generation_config.is_some());
        assert!(deserialized.system_instruction.is_some());
        assert!(deserialized.tools.is_some());
        assert!(deserialized.tool_config.is_some());
        assert!(deserialized.thinking_config.is_some());
    }
}
