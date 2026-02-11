//! Kiro (Amazon Q) transformer.
//!
//! Converts between the gaud OpenAI-compatible chat format and the Anthropic
//! Messages API format used by the Kiro gateway.  This mirrors the Python
//! `converters_anthropic.py` / `streaming_anthropic.py` logic in the
//! kiro-gateway project.

use serde_json::{json, Value};

use crate::providers::transformer::{
    ProviderResponseMeta, ProviderTransformer, StreamState,
    convert_tools_to_anthropic, convert_tool_choice, extract_system_message,
    filter_system_messages, normalize_stop_sequences, parse_image_url,
};
use crate::providers::types::*;
use crate::providers::ProviderError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Models exposed by the Kiro provider.
const SUPPORTED_MODELS: &[&str] = &[
    "kiro:auto",
    "kiro:claude-sonnet-4",
    "kiro:claude-sonnet-4.5",
    "kiro:claude-haiku-4.5",
    "kiro:claude-opus-4.5",
    "kiro:claude-3.7-sonnet",
];

/// Default max output tokens when not specified in the request.
const DEFAULT_MAX_TOKENS: u32 = 8192;

// ---------------------------------------------------------------------------
// KiroTransformer
// ---------------------------------------------------------------------------

/// Transformer for the Kiro (Amazon Q) provider.
///
/// Produces Anthropic Messages API JSON that the `kiro_gateway::KiroClient`
/// deserialises into its typed request structs.
pub struct KiroTransformer;

impl KiroTransformer {
    pub fn new() -> Self {
        Self
    }

    /// Strip the `kiro:` prefix from a model name.
    pub fn strip_model_prefix(model: &str) -> &str {
        model.strip_prefix("kiro:").unwrap_or(model)
    }

    // -- helpers used by transform_request ----------------------------------

    /// Convert a single OpenAI message to an Anthropic Messages API content block.
    fn convert_user_content(msg: &ChatMessage) -> Value {
        match &msg.content {
            Some(MessageContent::Text(t)) => {
                json!({
                    "role": "user",
                    "content": t,
                })
            }
            Some(MessageContent::Parts(parts)) => {
                let blocks: Vec<Value> = parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => {
                            if text.is_empty() {
                                None
                            } else {
                                Some(json!({"type": "text", "text": text}))
                            }
                        }
                        ContentPart::ImageUrl { image_url } => {
                            let (source_type, media_type, data) =
                                parse_image_url(&image_url.url);
                            if source_type == "base64" {
                                Some(json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
                                        "data": data,
                                    }
                                }))
                            } else {
                                // URLs can't be passed directly to the Anthropic
                                // Messages API — emit a text placeholder.
                                Some(json!({
                                    "type": "text",
                                    "text": format!("[Image: {}]", image_url.url),
                                }))
                            }
                        }
                    })
                    .collect();
                json!({
                    "role": "user",
                    "content": blocks,
                })
            }
            None => json!({"role": "user", "content": ""}),
        }
    }

    /// Convert an assistant message (with optional tool_calls) to Anthropic
    /// Messages format.
    fn convert_assistant_message(msg: &ChatMessage) -> Value {
        let mut blocks: Vec<Value> = Vec::new();

        // Text content.
        if let Some(ref content) = msg.content {
            let text = content.as_text();
            if !text.is_empty() {
                blocks.push(json!({"type": "text", "text": text}));
            }
        }

        // Tool calls → tool_use blocks.
        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                let input: Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(json!({}));
                blocks.push(json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.function.name,
                    "input": input,
                }));
            }
        }

        if blocks.is_empty() {
            json!({"role": "assistant", "content": ""})
        } else if blocks.len() == 1 && blocks[0].get("type").and_then(|t| t.as_str()) == Some("text") {
            // Single text block → use string shorthand.
            let text = blocks[0]["text"].as_str().unwrap_or_default();
            json!({"role": "assistant", "content": text})
        } else {
            json!({"role": "assistant", "content": blocks})
        }
    }

    /// Convert a tool-result message to an Anthropic `tool_result` content block.
    fn convert_tool_message(msg: &ChatMessage) -> Value {
        let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
        let text = msg
            .content
            .as_ref()
            .map(|c| c.as_text().to_string())
            .unwrap_or_default();

        // Infer error status from content (matches Python gateway heuristic).
        let is_error = text.starts_with("Error:")
            || text.starts_with("error:")
            || text.starts_with("ERROR:");

        json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_call_id,
                "content": text,
                "is_error": is_error,
            }]
        })
    }

    /// Merge adjacent messages with the same role.
    ///
    /// The Anthropic Messages API requires strictly alternating `user` / `assistant`
    /// roles.  This mirrors `merge_adjacent_messages` in the Python gateway
    /// (`converters_core.py`).
    fn merge_adjacent_messages(messages: Vec<Value>) -> Vec<Value> {
        let mut merged: Vec<Value> = Vec::with_capacity(messages.len());

        for msg in messages {
            let role = msg["role"].as_str().unwrap_or("").to_string();

            if let Some(last) = merged.last_mut() {
                let last_role = last["role"].as_str().unwrap_or("");
                if last_role == role {
                    // Same role — merge content.
                    let existing = last["content"].take();
                    let incoming = msg["content"].clone();

                    let mut blocks = Self::content_to_blocks(existing);
                    blocks.extend(Self::content_to_blocks(incoming));

                    last["content"] = Value::Array(blocks);
                    continue;
                }
            }

            merged.push(msg);
        }

        merged
    }

    /// Normalise content to a `Vec<Value>` of content blocks.
    ///
    /// If `content` is a string, wrap it in a `[{"type":"text","text":...}]`.
    /// If it is already an array, return as-is.
    fn content_to_blocks(content: Value) -> Vec<Value> {
        match content {
            Value::Array(arr) => arr,
            Value::String(s) if !s.is_empty() => {
                vec![json!({"type": "text", "text": s})]
            }
            _ => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// ProviderTransformer
// ---------------------------------------------------------------------------

impl ProviderTransformer for KiroTransformer {
    fn transform_request(&self, request: &ChatRequest) -> Result<Value, ProviderError> {
        let model = Self::strip_model_prefix(&request.model).to_string();
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        // System prompt.
        let system = extract_system_message(&request.messages);

        // Non-system messages.
        let filtered = filter_system_messages(&request.messages);
        let converted: Vec<Value> = filtered
            .iter()
            .map(|msg| match msg.role {
                MessageRole::User => Self::convert_user_content(msg),
                MessageRole::Assistant => Self::convert_assistant_message(msg),
                MessageRole::Tool => Self::convert_tool_message(msg),
                MessageRole::System => {
                    // Should not reach here after filter, but handle gracefully.
                    json!({"role": "user", "content": msg.content.as_ref().map(|c| c.as_text().to_string()).unwrap_or_default()})
                }
            })
            .collect();

        // Merge adjacent messages with the same role (Anthropic requires alternating
        // user/assistant). This mirrors `merge_adjacent_messages` from the Python gateway.
        let messages = Self::merge_adjacent_messages(converted);

        let stop_sequences = normalize_stop_sequences(&request.stop);

        // Tools.
        let tools = request.tools.as_ref().map(|ts| Value::Array(convert_tools_to_anthropic(ts)));

        // Tool choice.
        let tool_choice = convert_tool_choice(&request.tool_choice, None);

        // Build the request body.
        let mut body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": messages,
            "stream": false,
        });

        if let Some(sys) = system {
            body["system"] = Value::String(sys);
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(tp) = request.top_p {
            body["top_p"] = json!(tp);
        }
        if let Some(seqs) = stop_sequences {
            body["stop_sequences"] = json!(seqs);
        }
        if let Some(t) = tools {
            body["tools"] = t;
        }
        if let Some(tc) = tool_choice {
            body["tool_choice"] = tc;
        }

        Ok(body)
    }

    fn transform_response(
        &self,
        response: Value,
        meta: &ProviderResponseMeta,
    ) -> Result<ChatResponse, ProviderError> {
        let id = response["id"]
            .as_str()
            .unwrap_or("msg_unknown")
            .to_string();

        let content_blocks = response["content"].as_array();

        let mut content_text: Option<String> = None;
        let mut reasoning_text: Option<String> = None;
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        if let Some(blocks) = content_blocks {
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => {
                        let text = block["text"].as_str().unwrap_or_default();
                        content_text
                            .get_or_insert_with(String::new)
                            .push_str(text);
                    }
                    Some("tool_use") => {
                        tool_calls.push(ToolCall {
                            index: None,
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            r#type: "function".to_string(),
                            function: FunctionCall {
                                name: block["name"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                                arguments: {
                                    let default_input = json!({});
                                    serde_json::to_string(
                                        block.get("input").unwrap_or(&default_input),
                                    )
                                    .unwrap_or_default()
                                },
                            },
                        });
                    }
                    Some("thinking") => {
                        let thinking = block["thinking"].as_str().unwrap_or_default();
                        reasoning_text
                            .get_or_insert_with(String::new)
                            .push_str(thinking);
                    }
                    _ => {} // Ignore unknown block types.
                }
            }
        }

        let finish_reason = response["stop_reason"].as_str().map(|sr| {
            self.map_finish_reason(sr).to_string()
        });

        // Parse usage, including cache token details.
        let usage_val = &response["usage"];
        let prompt_tokens = usage_val["input_tokens"].as_u64().unwrap_or(0) as u32;
        let completion_tokens = usage_val["output_tokens"].as_u64().unwrap_or(0) as u32;

        let prompt_tokens_details = {
            let cache_creation = usage_val["cache_creation_input_tokens"].as_u64();
            let cache_read = usage_val["cache_read_input_tokens"].as_u64();
            if cache_creation.is_some() || cache_read.is_some() {
                Some(UsageTokenDetails {
                    cached_tokens: cache_read.map(|v| v as u32),
                    reasoning_tokens: None,
                })
            } else {
                None
            }
        };

        Ok(ChatResponse {
            id,
            object: "chat.completion".to_string(),
            created: meta.created,
            model: meta.model.clone(),
            choices: vec![Choice {
                index: 0,
                message: ResponseMessage {
                    role: "assistant".to_string(),
                    content: content_text,
                    reasoning_content: reasoning_text,
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                },
                finish_reason,
            }],
            usage: Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
                prompt_tokens_details,
                completion_tokens_details: None,
            },
        })
    }

    fn new_stream_state(&self, model: &str) -> Box<dyn StreamState> {
        Box::new(KiroStreamState::new(model))
    }

    fn provider_id(&self) -> &str {
        "kiro"
    }

    fn provider_name(&self) -> &str {
        "Kiro (Amazon Q)"
    }

    fn supports_model(&self, model: &str) -> bool {
        SUPPORTED_MODELS.iter().any(|m| *m == model)
    }

    fn supported_models(&self) -> Vec<String> {
        SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect()
    }

    fn default_max_tokens(&self) -> Option<u32> {
        Some(DEFAULT_MAX_TOKENS)
    }

    fn map_finish_reason(&self, reason: &str) -> &'static str {
        match reason {
            "end_turn" => "stop",
            "tool_use" => "tool_calls",
            "max_tokens" => "length",
            "stop_sequence" => "stop",
            _ => "stop",
        }
    }
}

// ---------------------------------------------------------------------------
// KiroStreamState
// ---------------------------------------------------------------------------

/// Per-stream state for converting Kiro/Anthropic SSE events to OpenAI chunks.
pub struct KiroStreamState {
    model: String,
    response_id: String,
    input_tokens: u32,
    output_tokens: u32,
    /// Next tool_call index to assign (OpenAI uses sequential indices).
    tool_call_index: u32,
    /// Index of the tool_call currently being streamed.
    current_tool_index: Option<u32>,
}

impl KiroStreamState {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            response_id: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            tool_call_index: 0,
            current_tool_index: None,
        }
    }

    fn make_chunk(
        &self,
        delta: Delta,
        finish_reason: Option<String>,
        usage: Option<Usage>,
    ) -> ChatChunk {
        ChatChunk {
            id: self.response_id.clone(),
            object: "chat.completion.chunk".to_string(),
            created: chrono::Utc::now().timestamp(),
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta,
                finish_reason,
            }],
            usage,
        }
    }

    fn map_finish_reason(reason: &str) -> &'static str {
        match reason {
            "end_turn" => "stop",
            "tool_use" => "tool_calls",
            "max_tokens" => "length",
            "stop_sequence" => "stop",
            _ => "stop",
        }
    }
}

impl StreamState for KiroStreamState {
    fn process_event(&mut self, data: &str) -> Result<Option<ChatChunk>, ProviderError> {
        let event: Value = serde_json::from_str(data)
            .map_err(|e| ProviderError::Other(format!("Kiro stream JSON parse error: {e}")))?;
        self.process_event_value(&event)
    }

    fn process_event_value(
        &mut self,
        event: &serde_json::Value,
    ) -> Result<Option<ChatChunk>, ProviderError> {

        let event_type = event["type"].as_str().unwrap_or("");

        match event_type {
            "message_start" => {
                let msg = &event["message"];
                self.response_id = msg["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                self.input_tokens = msg["usage"]["input_tokens"]
                    .as_u64()
                    .unwrap_or(0) as u32;

                Ok(Some(self.make_chunk(
                    Delta {
                        role: Some("assistant".to_string()),
                        content: None,
                        reasoning_content: None,
                        tool_calls: None,
                    },
                    None,
                    None,
                )))
            }

            "content_block_start" => {
                let block = &event["content_block"];
                let block_type = block["type"].as_str().unwrap_or("");

                if block_type == "tool_use" {
                    let idx = self.tool_call_index;
                    self.tool_call_index += 1;
                    self.current_tool_index = Some(idx);

                    let id = block["id"].as_str().unwrap_or_default().to_string();
                    let name = block["name"].as_str().unwrap_or_default().to_string();

                    return Ok(Some(self.make_chunk(
                        Delta {
                            role: None,
                            content: None,
                            reasoning_content: None,
                            tool_calls: Some(vec![ToolCall {
                                index: Some(idx),
                                id,
                                r#type: "function".to_string(),
                                function: FunctionCall {
                                    name,
                                    arguments: String::new(),
                                },
                            }]),
                        },
                        None,
                        None,
                    )));
                }

                Ok(None)
            }

            "content_block_delta" => {
                let delta = &event["delta"];
                let delta_type = delta["type"].as_str().unwrap_or("");

                match delta_type {
                    "text_delta" => {
                        let text = delta["text"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        Ok(Some(self.make_chunk(
                            Delta {
                                role: None,
                                content: Some(text),
                                reasoning_content: None,
                                tool_calls: None,
                            },
                            None,
                            None,
                        )))
                    }
                    "input_json_delta" => {
                        let partial = delta["partial_json"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        let tool_idx = self.current_tool_index.unwrap_or(0);

                        Ok(Some(self.make_chunk(
                            Delta {
                                role: None,
                                content: None,
                                reasoning_content: None,
                                tool_calls: Some(vec![ToolCall {
                                    index: Some(tool_idx),
                                    id: String::new(),
                                    r#type: "function".to_string(),
                                    function: FunctionCall {
                                        name: String::new(),
                                        arguments: partial,
                                    },
                                }]),
                            },
                            None,
                            None,
                        )))
                    }
                    "thinking_delta" => {
                        let thinking = delta["thinking"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        Ok(Some(self.make_chunk(
                            Delta {
                                role: None,
                                content: None,
                                reasoning_content: Some(thinking),
                                tool_calls: None,
                            },
                            None,
                            None,
                        )))
                    }
                    _ => Ok(None),
                }
            }

            "content_block_stop" => {
                self.current_tool_index = None;
                Ok(None)
            }

            "message_delta" => {
                let delta_obj = &event["delta"];
                let finish_reason = delta_obj["stop_reason"]
                    .as_str()
                    .map(|sr| Self::map_finish_reason(sr).to_string());

                // Capture output tokens.
                if let Some(tokens) = event["usage"]["output_tokens"].as_u64() {
                    self.output_tokens = tokens as u32;
                }

                let usage = Some(Usage {
                    prompt_tokens: self.input_tokens,
                    completion_tokens: self.output_tokens,
                    total_tokens: self.input_tokens + self.output_tokens,
                    prompt_tokens_details: None,
                    completion_tokens_details: None,
                });

                Ok(Some(self.make_chunk(
                    Delta {
                        role: None,
                        content: None,
                        reasoning_content: None,
                        tool_calls: None,
                    },
                    finish_reason,
                    usage,
                )))
            }

            "message_stop" | "ping" => Ok(None),

            "error" => {
                let error_type = event["error"]["type"]
                    .as_str()
                    .unwrap_or("unknown");
                let error_msg = event["error"]["message"]
                    .as_str()
                    .unwrap_or("Unknown error");
                tracing::warn!(
                    error_type = %error_type,
                    error_message = %error_msg,
                    "Kiro stream error event"
                );
                Err(ProviderError::Stream(format!(
                    "{error_type}: {error_msg}"
                )))
            }

            _ => {
                tracing::debug!(event_type, "Ignoring unknown Kiro stream event type");
                Ok(None)
            }
        }
    }

    fn final_usage(&self) -> Usage {
        Usage {
            prompt_tokens: self.input_tokens,
            completion_tokens: self.output_tokens,
            total_tokens: self.input_tokens + self.output_tokens,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        }
    }

    fn response_id(&self) -> &str {
        &self.response_id
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_transformer() -> KiroTransformer {
        KiroTransformer::new()
    }

    fn make_meta() -> ProviderResponseMeta {
        ProviderResponseMeta {
            provider: "kiro".to_string(),
            model: "kiro:claude-sonnet-4".to_string(),
            created: 1700000000,
            ..Default::default()
        }
    }

    // -- strip_model_prefix -------------------------------------------------

    #[test]
    fn test_strip_model_prefix() {
        assert_eq!(KiroTransformer::strip_model_prefix("kiro:claude-sonnet-4"), "claude-sonnet-4");
        assert_eq!(KiroTransformer::strip_model_prefix("claude-sonnet-4"), "claude-sonnet-4");
        assert_eq!(KiroTransformer::strip_model_prefix("kiro:auto"), "auto");
    }

    // -- transform_request --------------------------------------------------

    #[test]
    fn test_transform_request_basic() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:claude-sonnet-4".into(),
            messages: vec![
                ChatMessage {
                    role: MessageRole::System,
                    content: Some(MessageContent::Text("You are helpful.".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: MessageRole::User,
                    content: Some(MessageContent::Text("Hello".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            temperature: Some(0.7),
            max_tokens: Some(4096),
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        };

        let body = transformer.transform_request(&req).unwrap();
        assert_eq!(body["model"], "claude-sonnet-4");
        assert_eq!(body["max_tokens"], 4096);
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
        assert_eq!(body["system"], "You are helpful.");
        // Only user message (system extracted separately).
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "Hello");
    }

    #[test]
    fn test_transform_request_strips_kiro_prefix() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:claude-sonnet-4.5".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hi".into())),
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

        let body = transformer.transform_request(&req).unwrap();
        assert_eq!(body["model"], "claude-sonnet-4.5");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn test_transform_request_with_tools() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Search for rust".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: None,
            tools: Some(vec![Tool {
                r#type: "function".to_string(),
                function: FunctionDef {
                    name: "search".to_string(),
                    description: Some("Search the web".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"}
                        }
                    })),
                },
            }]),
            tool_choice: Some(json!("auto")),
            stream_options: None,
        };

        let body = transformer.transform_request(&req).unwrap();
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "search");
        assert_eq!(body["tool_choice"]["type"], "auto");
    }

    #[test]
    fn test_transform_request_tool_result() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::Tool,
                content: Some(MessageContent::Text("result data".into())),
                name: None,
                tool_calls: None,
                tool_call_id: Some("call_123".into()),
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

        let body = transformer.transform_request(&req).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "user");
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call_123");
        assert_eq!(content[0]["content"], "result data");
        assert_eq!(content[0]["is_error"], false);
    }

    #[test]
    fn test_transform_request_tool_result_error() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::Tool,
                content: Some(MessageContent::Text("Error: something failed".into())),
                name: None,
                tool_calls: None,
                tool_call_id: Some("call_456".into()),
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

        let body = transformer.transform_request(&req).unwrap();
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["is_error"], true);
    }

    #[test]
    fn test_transform_request_assistant_with_tool_calls() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::Assistant,
                content: None,
                name: None,
                tool_calls: Some(vec![ToolCall {
                    index: None,
                    id: "call_1".into(),
                    r#type: "function".into(),
                    function: FunctionCall {
                        name: "search".into(),
                        arguments: r#"{"q":"test"}"#.into(),
                    },
                }]),
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

        let body = transformer.transform_request(&req).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "assistant");
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["id"], "call_1");
        assert_eq!(content[0]["name"], "search");
        assert_eq!(content[0]["input"], json!({"q": "test"}));
    }

    #[test]
    fn test_transform_request_with_stop_sequences() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: Some(StopSequence::Multiple(vec!["END".into(), "STOP".into()])),
            tools: None,
            tool_choice: None,
            stream_options: None,
        };

        let body = transformer.transform_request(&req).unwrap();
        let seqs = body["stop_sequences"].as_array().unwrap();
        assert_eq!(seqs.len(), 2);
        assert_eq!(seqs[0], "END");
        assert_eq!(seqs[1], "STOP");
    }

    #[test]
    fn test_transform_request_multipart_image() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "What is this?".into(),
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "data:image/png;base64,iVBOR".into(),
                            detail: None,
                        },
                    },
                ])),
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

        let body = transformer.transform_request(&req).unwrap();
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "What is this?");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "iVBOR");
    }

    #[test]
    fn test_transform_request_skips_empty_text() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Parts(vec![
                    ContentPart::Text { text: "".into() },
                    ContentPart::Text {
                        text: "Actual text".into(),
                    },
                ])),
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

        let body = transformer.transform_request(&req).unwrap();
        let content = body["messages"][0]["content"].as_array().unwrap();
        // Empty text part should be filtered out.
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "Actual text");
    }

    // -- transform_response -------------------------------------------------

    #[test]
    fn test_transform_response_basic() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello there!"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        });

        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(result.id, "msg_123");
        assert_eq!(result.model, "kiro:claude-sonnet-4");
        assert_eq!(
            result.choices[0].message.content.as_deref(),
            Some("Hello there!")
        );
        assert_eq!(
            result.choices[0].finish_reason.as_deref(),
            Some("stop")
        );
        assert_eq!(result.usage.prompt_tokens, 10);
        assert_eq!(result.usage.completion_tokens, 5);
        assert_eq!(result.usage.total_tokens, 15);
    }

    #[test]
    fn test_transform_response_with_tool_use() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_tools",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Let me search."},
                {
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "search",
                    "input": {"q": "test"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 50, "output_tokens": 30}
        });

        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(
            result.choices[0].finish_reason.as_deref(),
            Some("tool_calls")
        );
        let tc = result.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id, "toolu_1");
        assert_eq!(tc[0].function.name, "search");
        assert_eq!(tc[0].function.arguments, r#"{"q":"test"}"#);
    }

    #[test]
    fn test_transform_response_with_thinking() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_think",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "Let me think about this..."},
                {"type": "text", "text": "The answer is 42."}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 20, "output_tokens": 30}
        });

        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(
            result.choices[0].message.content.as_deref(),
            Some("The answer is 42.")
        );
        assert_eq!(
            result.choices[0].message.reasoning_content.as_deref(),
            Some("Let me think about this...")
        );
    }

    #[test]
    fn test_transform_response_max_tokens() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_max",
            "type": "message",
            "content": [{"type": "text", "text": "partial..."}],
            "stop_reason": "max_tokens",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });

        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(
            result.choices[0].finish_reason.as_deref(),
            Some("length")
        );
    }

    #[test]
    fn test_transform_response_cache_tokens() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_cache",
            "type": "message",
            "content": [{"type": "text", "text": "cached"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_creation_input_tokens": 80,
                "cache_read_input_tokens": 20
            }
        });

        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        let details = result.usage.prompt_tokens_details.as_ref().unwrap();
        assert_eq!(details.cached_tokens, Some(20));
    }

    // -- stream state -------------------------------------------------------

    #[test]
    fn test_stream_state_message_start() {
        let mut state = KiroStreamState::new("kiro:claude-sonnet-4");
        let data = r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-4","usage":{"input_tokens":42}}}"#;
        let result = state.process_event(data).unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert_eq!(chunk.choices[0].delta.role.as_deref(), Some("assistant"));
        assert_eq!(state.response_id, "msg_1");
        assert_eq!(state.input_tokens, 42);
    }

    #[test]
    fn test_stream_state_text_delta() {
        let mut state = KiroStreamState::new("kiro:auto");
        // Initialize with message_start.
        state.process_event(r#"{"type":"message_start","message":{"id":"msg_s","model":"auto","usage":{"input_tokens":10}}}"#).unwrap();

        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let result = state.process_event(data).unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
        assert!(chunk.choices[0].delta.tool_calls.is_none());
    }

    #[test]
    fn test_stream_state_thinking_delta() {
        let mut state = KiroStreamState::new("kiro:auto");
        state.process_event(r#"{"type":"message_start","message":{"id":"msg_t","model":"auto","usage":{"input_tokens":5}}}"#).unwrap();

        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Thinking step..."}}"#;
        let result = state.process_event(data).unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert!(chunk.choices[0].delta.content.is_none());
        assert_eq!(
            chunk.choices[0].delta.reasoning_content.as_deref(),
            Some("Thinking step...")
        );
    }

    #[test]
    fn test_stream_state_tool_call() {
        let mut state = KiroStreamState::new("kiro:auto");
        state.process_event(r#"{"type":"message_start","message":{"id":"msg_tc","model":"auto","usage":{"input_tokens":20}}}"#).unwrap();

        // content_block_start with tool_use
        let start = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"get_weather","input":{}}}"#;
        let result = state.process_event(start).unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        let tc = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].index, Some(0));
        assert_eq!(tc[0].id, "toolu_abc");
        assert_eq!(tc[0].function.name, "get_weather");
        assert_eq!(tc[0].function.arguments, "");
        assert!(chunk.choices[0].delta.content.is_none());

        // input_json_delta
        let delta = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\": \"SF\"}"}}"#;
        let result = state.process_event(delta).unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        let tc = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].index, Some(0));
        assert_eq!(tc[0].function.arguments, r#"{"location": "SF"}"#);
        assert!(chunk.choices[0].delta.content.is_none());
    }

    #[test]
    fn test_stream_state_message_delta_usage() {
        let mut state = KiroStreamState::new("kiro:auto");
        state.process_event(r#"{"type":"message_start","message":{"id":"msg_u","model":"auto","usage":{"input_tokens":42}}}"#).unwrap();

        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}"#;
        let result = state.process_event(data).unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert_eq!(
            chunk.choices[0].finish_reason.as_deref(),
            Some("stop")
        );
        let usage = chunk.usage.as_ref().unwrap();
        assert_eq!(usage.prompt_tokens, 42);
        assert_eq!(usage.completion_tokens, 15);
        assert_eq!(usage.total_tokens, 57);

        // Verify final_usage.
        let final_usage = state.final_usage();
        assert_eq!(final_usage.prompt_tokens, 42);
        assert_eq!(final_usage.completion_tokens, 15);

        // Verify response_id.
        assert_eq!(state.response_id(), "msg_u");
    }

    #[test]
    fn test_stream_state_ping_returns_none() {
        let mut state = KiroStreamState::new("kiro:auto");
        let result = state.process_event(r#"{"type":"ping"}"#).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_stream_state_stop_returns_none() {
        let mut state = KiroStreamState::new("kiro:auto");
        let result = state.process_event(r#"{"type":"message_stop"}"#).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_stream_state_error() {
        let mut state = KiroStreamState::new("kiro:auto");
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let result = state.process_event(data);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::Stream(msg) => {
                assert!(msg.contains("overloaded_error"));
                assert!(msg.contains("Overloaded"));
            }
            other => panic!("Expected Stream error, got: {:?}", other),
        }
    }

    #[test]
    fn test_stream_state_content_block_stop_resets_tool() {
        let mut state = KiroStreamState::new("kiro:auto");
        state.current_tool_index = Some(0);
        let result = state.process_event(r#"{"type":"content_block_stop","index":0}"#).unwrap();
        assert!(result.is_none());
        assert!(state.current_tool_index.is_none());
    }

    #[test]
    fn test_merge_adjacent_messages() {
        let messages = vec![
            json!({"role": "user", "content": "Hello"}),
            json!({"role": "user", "content": "World"}),
            json!({"role": "assistant", "content": "Hi"}),
        ];
        let merged = KiroTransformer::merge_adjacent_messages(messages);
        assert_eq!(merged.len(), 2);
        // First message should have merged content as array of blocks.
        let blocks = merged[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], "Hello");
        assert_eq!(blocks[1]["text"], "World");
        // Second message unchanged.
        assert_eq!(merged[1]["role"], "assistant");
        assert_eq!(merged[1]["content"], "Hi");
    }

    // ========================================================================
    // Tests adapted from Python kiro-gateway test suite
    // Source: kiro-aws/kiro-gateway/tests/unit/
    // ========================================================================

    // -- merge_adjacent_messages (from test_converters_core.py) ---------------

    /// Adapted from TestMergeAdjacentMessages.test_merges_three_consecutive_user_messages
    #[test]
    fn test_merge_three_consecutive_user_messages() {
        let messages = vec![
            json!({"role": "user", "content": "Part 1"}),
            json!({"role": "user", "content": "Part 2"}),
            json!({"role": "user", "content": "Part 3"}),
        ];
        let merged = KiroTransformer::merge_adjacent_messages(messages);
        assert_eq!(merged.len(), 1);
        let blocks = merged[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["text"], "Part 1");
        assert_eq!(blocks[1]["text"], "Part 2");
        assert_eq!(blocks[2]["text"], "Part 3");
    }

    /// Adapted from TestMergeAdjacentMessages.test_does_not_merge_different_roles
    #[test]
    fn test_merge_does_not_merge_different_roles() {
        let messages = vec![
            json!({"role": "user", "content": "Hello"}),
            json!({"role": "assistant", "content": "Hi"}),
            json!({"role": "user", "content": "How are you?"}),
        ];
        let merged = KiroTransformer::merge_adjacent_messages(messages);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0]["content"], "Hello");
        assert_eq!(merged[1]["content"], "Hi");
        assert_eq!(merged[2]["content"], "How are you?");
    }

    /// Adapted from TestMergeAdjacentMessages.test_handles_empty_messages
    #[test]
    fn test_merge_handles_empty_messages() {
        let messages: Vec<Value> = vec![];
        let merged = KiroTransformer::merge_adjacent_messages(messages);
        assert!(merged.is_empty());
    }

    /// Adapted from TestMergeAdjacentMessages.test_handles_single_message
    #[test]
    fn test_merge_handles_single_message() {
        let messages = vec![json!({"role": "user", "content": "Only one"})];
        let merged = KiroTransformer::merge_adjacent_messages(messages);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0]["content"], "Only one");
    }

    /// Adapted from TestMergeAdjacentMessages.test_merges_adjacent_assistant_messages
    #[test]
    fn test_merge_adjacent_assistant_messages() {
        let messages = vec![
            json!({"role": "user", "content": "Question"}),
            json!({"role": "assistant", "content": "Part A"}),
            json!({"role": "assistant", "content": "Part B"}),
        ];
        let merged = KiroTransformer::merge_adjacent_messages(messages);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0]["content"], "Question");
        let blocks = merged[1]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
    }

    /// Adapted from TestMergeAdjacentMessages.test_preserves_content_with_array_blocks
    #[test]
    fn test_merge_preserves_content_with_array_blocks() {
        let messages = vec![
            json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "Hello"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "abc"}}
                ]
            }),
            json!({"role": "user", "content": "World"}),
        ];
        let merged = KiroTransformer::merge_adjacent_messages(messages);
        assert_eq!(merged.len(), 1);
        let blocks = merged[0]["content"].as_array().unwrap();
        // 2 from the array content + 1 from the string content converted to block
        assert!(blocks.len() >= 2);
    }

    /// Adapted from TestMergeAdjacentMessages.test_complex_alternating_pattern
    #[test]
    fn test_merge_complex_alternating_pattern() {
        let messages = vec![
            json!({"role": "user", "content": "U1"}),
            json!({"role": "user", "content": "U2"}),
            json!({"role": "assistant", "content": "A1"}),
            json!({"role": "assistant", "content": "A2"}),
            json!({"role": "user", "content": "U3"}),
        ];
        let merged = KiroTransformer::merge_adjacent_messages(messages);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0]["role"], "user");
        assert_eq!(merged[1]["role"], "assistant");
        assert_eq!(merged[2]["role"], "user");
        // U1 + U2 merged
        assert_eq!(merged[0]["content"].as_array().unwrap().len(), 2);
        // A1 + A2 merged
        assert_eq!(merged[1]["content"].as_array().unwrap().len(), 2);
        // U3 standalone
        assert_eq!(merged[2]["content"], "U3");
    }

    // -- model prefix stripping (from test_model_resolver.py) ----------------

    /// Adapted from TestNormalizeModelName.test_handles_empty_string
    #[test]
    fn test_strip_model_prefix_empty_string() {
        assert_eq!(KiroTransformer::strip_model_prefix(""), "");
    }

    /// Adapted from TestNormalizeModelName.test_passthrough_auto
    #[test]
    fn test_strip_model_prefix_auto() {
        assert_eq!(KiroTransformer::strip_model_prefix("kiro:auto"), "auto");
    }

    /// Adapted from TestNormalizeModelName.test_handles_unknown_format
    #[test]
    fn test_strip_model_prefix_no_prefix() {
        assert_eq!(
            KiroTransformer::strip_model_prefix("some-random-model"),
            "some-random-model"
        );
    }

    /// Adapted from TestNormalizeModelName - multiple prefix variants
    #[test]
    fn test_strip_model_prefix_various() {
        assert_eq!(
            KiroTransformer::strip_model_prefix("kiro:claude-haiku-4.5"),
            "claude-haiku-4.5"
        );
        assert_eq!(
            KiroTransformer::strip_model_prefix("kiro:claude-opus-4.5"),
            "claude-opus-4.5"
        );
        assert_eq!(
            KiroTransformer::strip_model_prefix("kiro:claude-3.7-sonnet"),
            "claude-3.7-sonnet"
        );
    }

    // -- transform_request: system prompt extraction -------------------------
    // Adapted from test_converters_anthropic.py::TestExtractSystemPrompt

    /// Adapted from TestExtractSystemPrompt.test_extracts_from_string
    #[test]
    fn test_system_prompt_extraction() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![
                ChatMessage {
                    role: MessageRole::System,
                    content: Some(MessageContent::Text("You are a helpful assistant.".into())),
                    name: None, tool_calls: None, tool_call_id: None,
                },
                ChatMessage {
                    role: MessageRole::User,
                    content: Some(MessageContent::Text("Hi".into())),
                    name: None, tool_calls: None, tool_call_id: None,
                },
            ],
            temperature: None, max_tokens: None, stream: false,
            top_p: None, stop: None, tools: None, tool_choice: None,
            stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        assert_eq!(body["system"], "You are a helpful assistant.");
    }

    /// Adapted from TestExtractSystemPrompt.test_handles_none
    #[test]
    fn test_no_system_prompt_produces_no_system_field() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hi".into())),
                name: None, tool_calls: None, tool_call_id: None,
            }],
            temperature: None, max_tokens: None, stream: false,
            top_p: None, stop: None, tools: None, tool_choice: None,
            stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        assert!(body.get("system").is_none() || body["system"].is_null());
    }

    // -- transform_request: default max_tokens ------
    // Adapted from test_converters_openai.py

    /// Verifies default max_tokens is applied when not specified
    #[test]
    fn test_default_max_tokens_applied() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hi".into())),
                name: None, tool_calls: None, tool_call_id: None,
            }],
            temperature: None, max_tokens: None, stream: false,
            top_p: None, stop: None, tools: None, tool_choice: None,
            stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    // -- transform_request: top_p parameter ------

    #[test]
    fn test_transform_request_with_top_p() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hi".into())),
                name: None, tool_calls: None, tool_call_id: None,
            }],
            temperature: None, max_tokens: None, stream: false,
            top_p: Some(0.95), stop: None, tools: None, tool_choice: None,
            stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        assert!((body["top_p"].as_f64().unwrap() - 0.95).abs() < 0.001);
    }

    /// Adapted from test_converters_openai.py - omitted parameters
    #[test]
    fn test_transform_request_omits_unset_params() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hi".into())),
                name: None, tool_calls: None, tool_call_id: None,
            }],
            temperature: None, max_tokens: None, stream: false,
            top_p: None, stop: None, tools: None, tool_choice: None,
            stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        // These should not be present when not set.
        assert!(body.get("temperature").is_none() || body["temperature"].is_null());
        assert!(body.get("top_p").is_none() || body["top_p"].is_null());
        assert!(body.get("stop_sequences").is_none() || body["stop_sequences"].is_null());
        assert!(body.get("tools").is_none() || body["tools"].is_null());
        assert!(body.get("tool_choice").is_none() || body["tool_choice"].is_null());
    }

    // -- transform_request: stop sequences -----------------------------------
    // Adapted from test_converters_core.py / test_converters_openai.py

    #[test]
    fn test_transform_request_single_stop_sequence() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![],
            temperature: None, max_tokens: None, stream: false,
            top_p: None,
            stop: Some(StopSequence::Single("HALT".into())),
            tools: None, tool_choice: None, stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        let seqs = body["stop_sequences"].as_array().unwrap();
        assert_eq!(seqs.len(), 1);
        assert_eq!(seqs[0], "HALT");
    }

    // -- transform_request: multiple tools -----------------------------------
    // Adapted from test_converters_anthropic.py::TestConvertAnthropicTools

    #[test]
    fn test_transform_request_multiple_tools() {
        let transformer = make_transformer();
        let tools = vec![
            Tool {
                r#type: "function".to_string(),
                function: FunctionDef {
                    name: "search".to_string(),
                    description: Some("Search the web".to_string()),
                    parameters: Some(json!({"type": "object", "properties": {"q": {"type": "string"}}})),
                },
            },
            Tool {
                r#type: "function".to_string(),
                function: FunctionDef {
                    name: "get_weather".to_string(),
                    description: Some("Get weather info".to_string()),
                    parameters: Some(json!({"type": "object", "properties": {"city": {"type": "string"}}})),
                },
            },
        ];
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Search and weather".into())),
                name: None, tool_calls: None, tool_call_id: None,
            }],
            temperature: None, max_tokens: None, stream: false,
            top_p: None, stop: None, tools: Some(tools), tool_choice: None,
            stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        let tools_val = body["tools"].as_array().unwrap();
        assert_eq!(tools_val.len(), 2);
        assert_eq!(tools_val[0]["name"], "search");
        assert_eq!(tools_val[1]["name"], "get_weather");
    }

    // -- transform_request: image edge cases ---------------------------------
    // Adapted from test_converters_core.py::TestExtractImagesFromContent

    /// Adapted from test_extracts_from_openai_format_data_url
    #[test]
    fn test_transform_request_image_jpeg() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Parts(vec![
                    ContentPart::Text { text: "Describe this".into() },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "data:image/jpeg;base64,/9j/4AAQ".into(),
                            detail: None,
                        },
                    },
                ])),
                name: None, tool_calls: None, tool_call_id: None,
            }],
            temperature: None, max_tokens: None, stream: false,
            top_p: None, stop: None, tools: None, tool_choice: None,
            stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["source"]["media_type"], "image/jpeg");
        assert_eq!(content[1]["source"]["data"], "/9j/4AAQ");
    }

    /// Adapted from test_extracts_gif_format / test_extracts_webp_format
    #[test]
    fn test_transform_request_image_gif_and_webp() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Parts(vec![
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "data:image/gif;base64,R0lGODlh".into(),
                            detail: None,
                        },
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "data:image/webp;base64,UklGRh4".into(),
                            detail: None,
                        },
                    },
                ])),
                name: None, tool_calls: None, tool_call_id: None,
            }],
            temperature: None, max_tokens: None, stream: false,
            top_p: None, stop: None, tools: None, tool_choice: None,
            stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["source"]["media_type"], "image/gif");
        assert_eq!(content[1]["source"]["media_type"], "image/webp");
    }

    // -- transform_response: stop_reason mapping ----------------------------
    // Adapted from test_streaming_anthropic.py / test_converters_anthropic.py

    /// Adapted from TestCollectAnthropicResponse.test_sets_stop_reason_end_turn
    #[test]
    fn test_transform_response_stop_reason_end_turn() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_stop",
            "content": [{"type": "text", "text": "Done"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(result.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    /// Adapted from TestCollectAnthropicResponse.test_sets_stop_reason_tool_use
    #[test]
    fn test_transform_response_stop_reason_tool_use() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_tu",
            "content": [{"type": "tool_use", "id": "t1", "name": "func", "input": {}}],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(result.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }

    /// Adapted from stop_reason mapping in Python gateway.
    /// Unknown stop reasons default to "stop" via the fallback arm.
    #[test]
    fn test_transform_response_unknown_stop_reason_defaults_to_stop() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_unk",
            "content": [{"type": "text", "text": "X"}],
            "stop_reason": "unknown_reason",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        // Unknown stop reasons fall through to "stop" (safe default).
        assert_eq!(result.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    /// When stop_reason is absent entirely, finish_reason should be None.
    #[test]
    fn test_transform_response_missing_stop_reason() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_no_sr",
            "content": [{"type": "text", "text": "X"}],
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert!(result.choices[0].finish_reason.is_none());
    }

    // -- transform_response: empty content ----------------------------------
    // Adapted from test_converters_anthropic.py

    #[test]
    fn test_transform_response_empty_content_blocks() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_empty",
            "content": [],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert!(result.choices[0].message.content.is_none());
        assert!(result.choices[0].message.tool_calls.is_none());
    }

    // -- transform_response: multiple tool_use blocks -----------------------
    // Adapted from test_converters_anthropic.py::TestConvertAnthropicMessages

    #[test]
    fn test_transform_response_multiple_tool_calls() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_mt",
            "content": [
                {"type": "tool_use", "id": "t1", "name": "search", "input": {"q": "rust"}},
                {"type": "tool_use", "id": "t2", "name": "weather", "input": {"city": "SF"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 50, "output_tokens": 30}
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        let tc = result.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 2);
        assert_eq!(tc[0].id, "t1");
        assert_eq!(tc[0].function.name, "search");
        assert_eq!(tc[1].id, "t2");
        assert_eq!(tc[1].function.name, "weather");
    }

    /// Adapted from test_converters_anthropic.py - text before tool_use
    #[test]
    fn test_transform_response_text_and_tool_use() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_tt",
            "content": [
                {"type": "text", "text": "I'll search for that."},
                {"type": "tool_use", "id": "t1", "name": "search", "input": {"q": "test"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 20}
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(result.choices[0].message.content.as_deref(), Some("I'll search for that."));
        let tc = result.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].function.name, "search");
    }

    // -- transform_response: thinking + text --------------------------------
    // Adapted from test_thinking_parser.py

    /// Adapted from TestThinkingParserFullFlow.test_complete_thinking_block
    #[test]
    fn test_transform_response_thinking_only() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_think_only",
            "content": [
                {"type": "thinking", "thinking": "Let me reason about this..."}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 10}
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(
            result.choices[0].message.reasoning_content.as_deref(),
            Some("Let me reason about this...")
        );
        // Content should be empty/None since only thinking block
        assert!(
            result.choices[0].message.content.as_deref() == Some("")
                || result.choices[0].message.content.is_none()
        );
    }

    /// Adapted from TestCollectAnthropicResponse.test_includes_model_name
    #[test]
    fn test_transform_response_preserves_model() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_m",
            "content": [{"type": "text", "text": "Hi"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let meta = ProviderResponseMeta {
            provider: "kiro".to_string(),
            model: "kiro:claude-haiku-4.5".to_string(),
            created: 1700000000,
            ..Default::default()
        };
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(result.model, "kiro:claude-haiku-4.5");
    }

    /// Adapted from TestCollectAnthropicResponse.test_generates_message_id
    #[test]
    fn test_transform_response_preserves_id() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_abcdef123456",
            "content": [{"type": "text", "text": "Hi"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(result.id, "msg_abcdef123456");
    }

    // -- transform_response: cache tokens -----------------------------------
    // Adapted from test_converters_anthropic.py / test_streaming_anthropic.py

    /// Adapted from test_includes_usage_info
    #[test]
    fn test_transform_response_cache_creation_only() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_cc",
            "content": [{"type": "text", "text": "cached"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_creation_input_tokens": 80
            }
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert!(result.usage.prompt_tokens_details.is_some());
    }

    /// No cache tokens at all should produce None for details
    #[test]
    fn test_transform_response_no_cache_tokens() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_nc",
            "content": [{"type": "text", "text": "no cache"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert!(result.usage.prompt_tokens_details.is_none());
    }

    // -- stream state: lifecycle events -------------------------------------
    // Adapted from test_streaming_anthropic.py

    /// Adapted from TestStreamKiroToAnthropic.test_yields_message_start_event
    #[test]
    fn test_stream_state_message_start_sets_response_id() {
        let mut state = KiroStreamState::new("kiro:auto");
        let data = r#"{"type":"message_start","message":{"id":"msg_lifecycle","model":"auto","usage":{"input_tokens":99}}}"#;
        let result = state.process_event(data).unwrap();
        assert!(result.is_some());
        assert_eq!(state.response_id, "msg_lifecycle");
        assert_eq!(state.input_tokens, 99);
    }

    /// Adapted from TestStreamKiroToAnthropic.test_yields_content_block_delta_for_content
    #[test]
    fn test_stream_state_multiple_text_deltas() {
        let mut state = KiroStreamState::new("kiro:auto");
        state.process_event(r#"{"type":"message_start","message":{"id":"msg_md","model":"auto","usage":{"input_tokens":0}}}"#).unwrap();

        let r1 = state.process_event(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#).unwrap();
        let r2 = state.process_event(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" World"}}"#).unwrap();

        assert_eq!(r1.unwrap().choices[0].delta.content.as_deref(), Some("Hello"));
        assert_eq!(r2.unwrap().choices[0].delta.content.as_deref(), Some(" World"));
    }

    /// Adapted from TestStreamKiroToAnthropic.test_stop_reason_is_tool_use_when_tools_present
    #[test]
    fn test_stream_state_stop_reason_tool_use() {
        let mut state = KiroStreamState::new("kiro:auto");
        state.process_event(r#"{"type":"message_start","message":{"id":"msg_sr","model":"auto","usage":{"input_tokens":0}}}"#).unwrap();

        let data = r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":10}}"#;
        let result = state.process_event(data).unwrap();
        assert!(result.is_some());
        let chunk = result.unwrap();
        assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }

    /// Adapted from TestStreamKiroToAnthropic.test_yields_tool_use_block_for_tool_calls
    #[test]
    fn test_stream_state_tool_use_full_lifecycle() {
        let mut state = KiroStreamState::new("kiro:auto");
        state.process_event(r#"{"type":"message_start","message":{"id":"msg_tl","model":"auto","usage":{"input_tokens":0}}}"#).unwrap();

        // Tool start
        let start = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_xyz","name":"get_weather","input":{}}}"#;
        let r1 = state.process_event(start).unwrap().unwrap();
        let tc = r1.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].id, "toolu_xyz");
        assert_eq!(tc[0].function.name, "get_weather");

        // JSON delta
        let delta = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"city\":\"SF\"}"}}"#;
        let r2 = state.process_event(delta).unwrap().unwrap();
        let tc2 = r2.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tc2[0].function.arguments, r#"{"city":"SF"}"#);

        // Block stop
        let stop = r#"{"type":"content_block_stop","index":0}"#;
        let r3 = state.process_event(stop).unwrap();
        assert!(r3.is_none());
        assert!(state.current_tool_index.is_none());
    }

    /// Adapted from TestStreamKiroToAnthropic - handles Unicode content
    #[test]
    fn test_stream_state_unicode_content() {
        let mut state = KiroStreamState::new("kiro:auto");
        state.process_event(r#"{"type":"message_start","message":{"id":"msg_uc","model":"auto","usage":{"input_tokens":0}}}"#).unwrap();

        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Привет мир! 🌍"}}"#;
        let result = state.process_event(data).unwrap();
        assert!(result.is_some());
        let content = result.unwrap().choices[0].delta.content.clone().unwrap();
        assert!(content.contains("Привет"));
        assert!(content.contains("🌍"));
    }

    /// Adapted from test_streaming_anthropic.py - error during stream
    #[test]
    fn test_stream_state_error_with_message() {
        let mut state = KiroStreamState::new("kiro:auto");
        let data = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Rate limit exceeded"}}"#;
        let result = state.process_event(data);
        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("rate_limit_error"));
        assert!(err_msg.contains("Rate limit exceeded"));
    }

    /// Adapted from stream lifecycle - thinking delta in stream
    #[test]
    fn test_stream_state_thinking_content_preserved() {
        let mut state = KiroStreamState::new("kiro:auto");
        state.process_event(r#"{"type":"message_start","message":{"id":"msg_tp","model":"auto","usage":{"input_tokens":0}}}"#).unwrap();

        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Step 1: Analyze. Step 2: Conclude."}}"#;
        let result = state.process_event(data).unwrap().unwrap();
        assert_eq!(
            result.choices[0].delta.reasoning_content.as_deref(),
            Some("Step 1: Analyze. Step 2: Conclude.")
        );
        assert!(result.choices[0].delta.content.is_none());
    }

    /// Adapted from test_streaming_anthropic.py - message_delta usage tracking
    #[test]
    fn test_stream_state_usage_accumulation() {
        let mut state = KiroStreamState::new("kiro:auto");
        // Set input tokens via message_start
        state.process_event(
            r#"{"type":"message_start","message":{"id":"msg_ua","model":"auto","usage":{"input_tokens":100}}}"#
        ).unwrap();

        // Set output tokens via message_delta
        let delta = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":50}}"#;
        state.process_event(delta).unwrap();

        let usage = state.final_usage();
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    // -- transform_request: ensures stream=false in body --------------------

    #[test]
    fn test_transform_request_sets_stream_false() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hi".into())),
                name: None, tool_calls: None, tool_call_id: None,
            }],
            temperature: None, max_tokens: None, stream: true,
            top_p: None, stop: None, tools: None, tool_choice: None,
            stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        // Body should have stream: false regardless of request.stream
        assert_eq!(body["stream"], false);
    }

    // -- transform_request: assistant without tool_calls --------------------

    #[test]
    fn test_transform_request_assistant_text_only() {
        let transformer = make_transformer();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::Assistant,
                content: Some(MessageContent::Text("I understand.".into())),
                name: None, tool_calls: None, tool_call_id: None,
            }],
            temperature: None, max_tokens: None, stream: false,
            top_p: None, stop: None, tools: None, tool_choice: None,
            stream_options: None,
        };
        let body = transformer.transform_request(&req).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["content"], "I understand.");
    }

    // -- SUPPORTED_MODELS constant ------------------------------------------

    #[test]
    fn test_supported_models_contains_expected() {
        assert!(SUPPORTED_MODELS.contains(&"kiro:auto"));
        assert!(SUPPORTED_MODELS.contains(&"kiro:claude-sonnet-4"));
        assert!(SUPPORTED_MODELS.contains(&"kiro:claude-sonnet-4.5"));
        assert!(SUPPORTED_MODELS.contains(&"kiro:claude-haiku-4.5"));
        assert!(SUPPORTED_MODELS.contains(&"kiro:claude-opus-4.5"));
        assert!(SUPPORTED_MODELS.contains(&"kiro:claude-3.7-sonnet"));
    }

    // -- transform_response: object field -----------------------------------

    #[test]
    fn test_transform_response_object_field() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_obj",
            "content": [{"type": "text", "text": "Hi"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(result.object, "chat.completion");
    }

    /// Adapted from test_converters_anthropic.py - created timestamp preserved
    #[test]
    fn test_transform_response_created_timestamp() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_ts",
            "content": [{"type": "text", "text": "Hi"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });
        let meta = ProviderResponseMeta {
            provider: "kiro".to_string(),
            model: "kiro:auto".to_string(),
            created: 1234567890,
            ..Default::default()
        };
        let result = transformer.transform_response(response, &meta).unwrap();
        assert_eq!(result.created, 1234567890);
    }

    // -- stream state: unknown event types ----------------------------------

    #[test]
    fn test_stream_state_ignores_unknown_event_types() {
        let mut state = KiroStreamState::new("kiro:auto");
        let result = state.process_event(r#"{"type":"custom_event","data":"stuff"}"#).unwrap();
        assert!(result.is_none());
    }

    // -- stream state: content_block_start with text type -------------------

    #[test]
    fn test_stream_state_content_block_start_text() {
        let mut state = KiroStreamState::new("kiro:auto");
        state.process_event(r#"{"type":"message_start","message":{"id":"msg_cbt","model":"auto","usage":{"input_tokens":0}}}"#).unwrap();

        // Text content_block_start should return None (actual content comes in deltas)
        let result = state.process_event(r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#).unwrap();
        assert!(result.is_none());
    }
}
