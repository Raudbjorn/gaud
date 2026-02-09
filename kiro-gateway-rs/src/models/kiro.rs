//! Raw Kiro API payload types.

use serde::{Deserialize, Serialize};

/// Complete Kiro API payload for generateAssistantResponse.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroPayload {
    pub conversation_state: ConversationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_arn: Option<String>,
}

/// Conversation state within a Kiro payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationState {
    pub chat_trigger_type: String,
    pub conversation_id: String,
    pub current_message: CurrentMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<HistoryEntry>>,
}

/// Wrapper for the current message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentMessage {
    pub user_input_message: UserInputMessage,
}

/// User input message in Kiro format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputMessage {
    pub content: String,
    pub model_id: String,
    pub origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<KiroImage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_input_message_context: Option<UserInputMessageContext>,
}

/// Context attached to a user input message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputMessageContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<KiroToolSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_results: Option<Vec<KiroToolResult>>,
}

/// Tool specification in Kiro format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroToolSpec {
    pub tool_specification: ToolSpecification,
}

/// Inner tool specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpecification {
    pub name: String,
    pub description: String,
    pub input_schema: InputSchema,
}

/// Tool input schema wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSchema {
    pub json: serde_json::Value,
}

/// Tool result in Kiro format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroToolResult {
    pub content: Vec<KiroTextContent>,
    pub status: String,
    pub tool_use_id: String,
}

/// Text content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KiroTextContent {
    pub text: String,
}

/// Image in Kiro format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KiroImage {
    pub format: String,
    pub source: KiroImageSource,
}

/// Image source (base64 bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KiroImageSource {
    pub bytes: String,
}

/// History entry - either user input or assistant response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryEntry {
    UserInputMessage(UserInputMessage),
    AssistantResponseMessage(AssistantResponseMessage),
}

// Custom serialization: Kiro expects `{"userInputMessage": {...}}` or `{"assistantResponseMessage": {...}}`
// The default enum serialization would use `{"UserInputMessage": {...}}`.
// We handle this by using a wrapper approach in the conversion layer instead.

/// Assistant response message in history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantResponseMessage {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_uses: Option<Vec<KiroToolUse>>,
}

/// Tool use in an assistant response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroToolUse {
    pub name: String,
    pub input: serde_json::Value,
    pub tool_use_id: String,
}

/// Parsed event from Kiro's AWS SSE stream.
#[derive(Debug, Clone)]
pub enum KiroStreamEvent {
    /// Text content chunk.
    Content(String),
    /// Tool call start.
    ToolStart {
        name: String,
        tool_use_id: String,
        input: String,
    },
    /// Tool call input continuation.
    ToolInput(String),
    /// Tool call end.
    ToolStop,
    /// Usage/metering data.
    Usage(serde_json::Value),
    /// Context usage percentage.
    ContextUsage(f64),
}

/// Model info from ListAvailableModels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroModelInfo {
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u32>,
}
