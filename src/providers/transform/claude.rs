use crate::providers::ProviderError;
use crate::providers::transform::util;
use crate::providers::transformer::{ProviderResponseMeta, ProviderTransformer, StreamState};
use crate::providers::types::*;
use serde_json::{Value, json};

const SUPPORTED_MODELS: &[&str] = &[
    "claude-sonnet-4-20250514",
    "claude-haiku-4-20250514",
    "claude-opus-4-20250514",
    "claude-3-5-sonnet-20241022",
    "claude-3-5-haiku-20241022",
];

pub struct ClaudeTransformer;

impl ClaudeTransformer {
    pub fn new() -> Self {
        Self
    }

    fn convert_message_content(content: &MessageContent) -> Value {
        match content {
            MessageContent::Text(text) => {
                if text.is_empty() {
                    json!([])
                } else {
                    json!([{"type": "text", "text": text}])
                }
            }
            MessageContent::Parts(parts) => {
                let blocks: Vec<Value> = parts
                    .iter()
                    .filter_map(|part| Self::convert_content_part(part))
                    .collect();
                json!(blocks)
            }
        }
    }

    fn convert_content_part(part: &ContentPart) -> Option<Value> {
        match part {
            ContentPart::Text { text } => {
                if text.is_empty() {
                    None
                } else {
                    Some(json!({"type": "text", "text": text}))
                }
            }
            ContentPart::ImageUrl { image_url } => {
                let (source_type, media_type, data) = util::parse_image_url(&image_url.url);
                if source_type == "url" {
                    Some(json!({
                        "type": "image",
                        "source": {
                            "type": "url",
                            "url": data
                        }
                    }))
                } else {
                    Some(json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": data
                        }
                    }))
                }
            }
        }
    }

    fn convert_messages(messages: &[ChatMessage]) -> Result<Vec<Value>, ProviderError> {
        let filtered = util::filter_system_messages(messages);
        let alternated = util::enforce_message_alternation(filtered);

        let mut result = Vec::new();

        for msg in &alternated {
            match msg.role {
                MessageRole::Tool => {
                    let tool_call_id = msg.tool_call_id.as_deref().unwrap_or("");
                    let sanitized_id = util::sanitize_tool_call_id(tool_call_id);
                    let content_text = msg
                        .content
                        .as_ref()
                        .map(|c| match c {
                            MessageContent::Text(t) => t.clone(),
                            MessageContent::Parts(parts) => parts
                                .iter()
                                .filter_map(|p| match p {
                                    ContentPart::Text { text } => Some(text.clone()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join(""),
                        })
                        .unwrap_or_default();

                    result.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": sanitized_id,
                            "content": content_text
                        }]
                    }));
                }
                MessageRole::Assistant => {
                    let mut content_blocks: Vec<Value> = Vec::new();

                    if let Some(ref content) = msg.content {
                        match content {
                            MessageContent::Text(text) => {
                                if !text.is_empty() {
                                    content_blocks.push(json!({"type": "text", "text": text}));
                                }
                            }
                            MessageContent::Parts(parts) => {
                                for part in parts {
                                    if let Some(block) = Self::convert_content_part(part) {
                                        content_blocks.push(block);
                                    }
                                }
                            }
                        }
                    }

                    if let Some(ref tool_calls) = msg.tool_calls {
                        for tc in tool_calls {
                            let input: Value =
                                serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                            content_blocks.push(json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.function.name,
                                "input": input
                            }));
                        }
                    }

                    if content_blocks.is_empty() {
                        content_blocks.push(json!({"type": "text", "text": " "}));
                    }

                    result.push(json!({
                        "role": "assistant",
                        "content": content_blocks
                    }));
                }
                MessageRole::User => {
                    let content = msg
                        .content
                        .as_ref()
                        .map(|c| Self::convert_message_content(c))
                        .unwrap_or_else(|| json!([]));

                    result.push(json!({
                        "role": "user",
                        "content": content
                    }));
                }
                MessageRole::System => {
                    // System messages are handled separately via concatenate_system_messages;
                    // this branch should not be reached after filter_system_messages, but
                    // handle defensively.
                }
            }
        }

        Ok(result)
    }
}

impl ProviderTransformer for ClaudeTransformer {
    fn transform_request(&self, request: &ChatRequest) -> Result<Value, ProviderError> {
        let system_text = util::concatenate_system_messages(&request.messages);
        let messages = Self::convert_messages(&request.messages)?;

        let max_tokens = request
            .max_tokens
            .or(self.default_max_tokens())
            .unwrap_or(8192);

        let mut body = json!({
            "model": request.model,
            "max_tokens": max_tokens,
            "messages": messages,
        });

        if let Some(ref system) = system_text {
            body["system"] = json!(system);
        }

        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }

        if let Some(top_p) = request.top_p {
            body["top_p"] = json!(top_p);
        }

        if let Some(ref stop) = request.stop {
            if let Some(sequences) = util::normalize_stop_sequences(&Some(stop.clone())) {
                body["stop_sequences"] = json!(sequences);
            }
        }

        if let Some(ref tools) = request.tools {
            if !tools.is_empty() {
                body["tools"] = json!(util::convert_tools_to_anthropic(tools));

                if let Some(ref tool_choice) = request.tool_choice {
                    if let Some(tc) = util::convert_tool_choice(&Some(tool_choice.clone()), None) {
                        body["tool_choice"] = tc;
                    }
                }
            }
        }

        if request.stream {
            body["stream"] = json!(true);
        }

        Ok(body)
    }

    fn transform_response(
        &self,
        response: Value,
        meta: &ProviderResponseMeta,
    ) -> Result<ChatResponse, ProviderError> {
        let id = response["id"].as_str().unwrap_or("msg_unknown").to_string();

        let model = response["model"]
            .as_str()
            .unwrap_or(&meta.model)
            .to_string();

        let stop_reason = response["stop_reason"].as_str().unwrap_or("end_turn");
        let finish_reason = self.map_finish_reason(stop_reason).to_string();

        let mut content: Option<String> = None;
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        if let Some(blocks) = response["content"].as_array() {
            let mut text_parts: Vec<String> = Vec::new();
            let mut tool_index: u32 = 0;

            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = block["text"].as_str() {
                            text_parts.push(text.to_string());
                        }
                    }
                    Some("tool_use") => {
                        let tc_id = block["id"].as_str().unwrap_or("").to_string();
                        let name = block["name"].as_str().unwrap_or("").to_string();
                        let input = &block["input"];

                        tool_calls.push(util::convert_anthropic_tool_use_to_openai(
                            &tc_id,
                            &name,
                            input,
                            Some(tool_index),
                        ));
                        tool_index += 1;
                    }
                    _ => {}
                }
            }

            if !text_parts.is_empty() {
                content = Some(text_parts.join(""));
            }
        }

        let usage_obj = &response["usage"];
        let input_tokens = usage_obj["input_tokens"].as_u64().unwrap_or(0) as u32;
        let output_tokens = usage_obj["output_tokens"].as_u64().unwrap_or(0) as u32;
        let cache_creation = usage_obj["cache_creation_input_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;
        let cache_read = usage_obj["cache_read_input_tokens"].as_u64().unwrap_or(0) as u32;

        let prompt_tokens_details = if cache_read > 0 || cache_creation > 0 {
            Some(UsageTokenDetails {
                cached_tokens: Some(cache_read),
                reasoning_tokens: None,
            })
        } else {
            None
        };

        let usage = Usage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens + output_tokens,
            prompt_tokens_details,
            completion_tokens_details: None,
        };

        let message = ResponseMessage {
            role: "assistant".to_string(),
            content,
            reasoning_content: None,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
        };

        let choice = Choice {
            index: 0,
            message,
            finish_reason: Some(finish_reason),
        };

        Ok(ChatResponse {
            id,
            object: "chat.completion".to_string(),
            created: meta.created,
            model,
            choices: vec![choice],
            usage,
        })
    }

    fn new_stream_state(&self, model: &str) -> Box<dyn StreamState> {
        Box::new(ClaudeStreamState::new(model))
    }

    fn provider_id(&self) -> &str {
        "anthropic"
    }

    fn provider_name(&self) -> &str {
        "Anthropic"
    }

    fn supports_model(&self, model: &str) -> bool {
        SUPPORTED_MODELS.contains(&model)
    }

    fn supported_models(&self) -> Vec<String> {
        SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect()
    }

    fn default_max_tokens(&self) -> Option<u32> {
        Some(8192)
    }

    fn map_finish_reason(&self, reason: &str) -> &'static str {
        util::map_finish_reason_to_openai(reason)
    }
}

pub struct ClaudeStreamState {
    response_id: String,
    model: String,
    tool_index: i32,
    input_tokens: u32,
    output_tokens: u32,
    finish_reason: Option<String>,
}

impl ClaudeStreamState {
    pub fn new(model: &str) -> Self {
        Self {
            response_id: String::new(),
            model: model.to_string(),
            tool_index: -1,
            input_tokens: 0,
            output_tokens: 0,
            finish_reason: None,
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
}

impl StreamState for ClaudeStreamState {
    fn process_event(&mut self, data: &str) -> Result<Option<ChatChunk>, ProviderError> {
        let event: Value = serde_json::from_str(data).map_err(|e| {
            ProviderError::ResponseParsing(format!("Failed to parse stream event: {}", e))
        })?;

        let event_type = event["type"].as_str().unwrap_or("");

        match event_type {
            "message_start" => {
                if let Some(message) = event.get("message") {
                    self.response_id = message["id"].as_str().unwrap_or("").to_string();

                    if let Some(model) = message["model"].as_str() {
                        self.model = model.to_string();
                    }

                    if let Some(usage) = message.get("usage") {
                        self.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0) as u32;
                    }
                }

                let delta = Delta {
                    role: Some("assistant".to_string()),
                    content: None,
                    reasoning_content: None,
                    tool_calls: None,
                };

                Ok(Some(self.make_chunk(delta, None, None)))
            }

            "content_block_start" => {
                if let Some(content_block) = event.get("content_block") {
                    let block_type = content_block["type"].as_str().unwrap_or("");

                    match block_type {
                        "tool_use" => {
                            self.tool_index += 1;

                            let id = content_block["id"].as_str().unwrap_or("").to_string();
                            let name = content_block["name"].as_str().unwrap_or("").to_string();

                            let tool_call = ToolCall {
                                index: Some(self.tool_index as u32),
                                id: util::sanitize_tool_call_id(&id),
                                r#type: "function".to_string(),
                                function: FunctionCall {
                                    name,
                                    arguments: String::new(),
                                },
                            };

                            let delta = Delta {
                                role: None,
                                content: None,
                                reasoning_content: None,
                                tool_calls: Some(vec![tool_call]),
                            };

                            Ok(Some(self.make_chunk(delta, None, None)))
                        }
                        "text" => {
                            // Text block start, no content to emit yet
                            Ok(None)
                        }
                        _ => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            }

            "content_block_delta" => {
                if let Some(delta_obj) = event.get("delta") {
                    let delta_type = delta_obj["type"].as_str().unwrap_or("");

                    match delta_type {
                        "text_delta" => {
                            let text = delta_obj["text"].as_str().unwrap_or("").to_string();

                            let delta = Delta {
                                role: None,
                                content: Some(text),
                                reasoning_content: None,
                                tool_calls: None,
                            };

                            Ok(Some(self.make_chunk(delta, None, None)))
                        }
                        "input_json_delta" => {
                            let partial_json =
                                delta_obj["partial_json"].as_str().unwrap_or("").to_string();

                            let tool_call = ToolCall {
                                index: Some(self.tool_index as u32),
                                id: String::new(),
                                r#type: "function".to_string(),
                                function: FunctionCall {
                                    name: String::new(),
                                    arguments: partial_json,
                                },
                            };

                            let delta = Delta {
                                role: None,
                                content: None,
                                reasoning_content: None,
                                tool_calls: Some(vec![tool_call]),
                            };

                            Ok(Some(self.make_chunk(delta, None, None)))
                        }
                        _ => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            }

            "content_block_stop" => Ok(None),

            "message_delta" => {
                if let Some(delta_obj) = event.get("delta") {
                    if let Some(reason) = delta_obj["stop_reason"].as_str() {
                        self.finish_reason =
                            Some(util::map_finish_reason_to_openai(reason).to_string());
                    }
                }

                if let Some(usage) = event.get("usage") {
                    self.output_tokens = usage["output_tokens"].as_u64().unwrap_or(0) as u32;
                }

                let finish_reason = self.finish_reason.clone();
                let usage = Usage {
                    prompt_tokens: self.input_tokens,
                    completion_tokens: self.output_tokens,
                    total_tokens: self.input_tokens + self.output_tokens,
                    prompt_tokens_details: None,
                    completion_tokens_details: None,
                };

                let delta = Delta {
                    role: None,
                    content: None,
                    reasoning_content: None,
                    tool_calls: None,
                };

                Ok(Some(self.make_chunk(delta, finish_reason, Some(usage))))
            }

            "message_stop" => Ok(None),

            "ping" => Ok(None),

            "error" => {
                let error_msg = event["error"]["message"]
                    .as_str()
                    .unwrap_or("Unknown stream error");
                Err(ProviderError::Stream(error_msg.to_string()))
            }

            _ => Ok(None),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_transformer() -> ClaudeTransformer {
        ClaudeTransformer::new()
    }

    fn make_user_message(text: &str) -> ChatMessage {
        ChatMessage {
            role: MessageRole::User,
            content: Some(MessageContent::Text(text.to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn make_system_message(text: &str) -> ChatMessage {
        ChatMessage {
            role: MessageRole::System,
            content: Some(MessageContent::Text(text.to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn make_basic_request(messages: Vec<ChatMessage>) -> ChatRequest {
        ChatRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            messages,
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        }
    }

    fn make_meta() -> ProviderResponseMeta {
        ProviderResponseMeta {
            model: "claude-sonnet-4-20250514".to_string(),
            created: 1700000000,
            ..Default::default()
        }
    }

    #[test]
    fn test_transform_request_basic() {
        let transformer = make_transformer();
        let request = make_basic_request(vec![make_user_message("Hello")]);

        let result = transformer.transform_request(&request).unwrap();

        assert_eq!(result["model"], "claude-sonnet-4-20250514");
        assert_eq!(result["max_tokens"], 8192);

        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");

        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Hello");

        // No system field when no system message
        assert!(result.get("system").is_none());
    }

    #[test]
    fn test_transform_request_with_system() {
        let transformer = make_transformer();
        let request = make_basic_request(vec![
            make_system_message("You are helpful."),
            make_user_message("Hi"),
        ]);

        let result = transformer.transform_request(&request).unwrap();

        assert_eq!(result["system"], "You are helpful.");

        let messages = result["messages"].as_array().unwrap();
        // System message should be filtered out of messages array
        for msg in messages {
            assert_ne!(msg["role"], "system");
        }
    }

    #[test]
    fn test_transform_request_with_tools() {
        let transformer = make_transformer();
        let tool = Tool {
            r#type: "function".to_string(),
            function: FunctionDef {
                name: "get_weather".to_string(),
                description: Some("Get weather info".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "location": {"type": "string"}
                    }
                })),
            },
        };

        let mut request = make_basic_request(vec![make_user_message("What's the weather?")]);
        request.tools = Some(vec![tool]);

        let result = transformer.transform_request(&request).unwrap();

        let tools = result["tools"].as_array().unwrap();
        assert!(!tools.is_empty());
        assert_eq!(tools[0]["name"], "get_weather");
    }

    #[test]
    fn test_transform_request_skips_empty_text() {
        let transformer = make_transformer();
        let request = make_basic_request(vec![ChatMessage {
            role: MessageRole::User,
            content: Some(MessageContent::Parts(vec![
                ContentPart::Text {
                    text: String::new(),
                },
                ContentPart::Text {
                    text: "Hello".to_string(),
                },
            ])),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }]);

        let result = transformer.transform_request(&request).unwrap();

        let messages = result["messages"].as_array().unwrap();
        let content = messages[0]["content"].as_array().unwrap();

        // Empty text block should be filtered out
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "Hello");
    }

    #[test]
    fn test_transform_request_tool_result() {
        let transformer = make_transformer();
        let messages = vec![
            make_user_message("Use the tool"),
            ChatMessage {
                role: MessageRole::Assistant,
                content: None,
                name: None,
                tool_calls: Some(vec![ToolCall {
                    index: Some(0),
                    id: "toolu_abc123".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: "get_weather".to_string(),
                        arguments: r#"{"location":"NYC"}"#.to_string(),
                    },
                }]),
                tool_call_id: None,
            },
            ChatMessage {
                role: MessageRole::Tool,
                content: Some(MessageContent::Text("Sunny, 72F".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: Some("toolu_abc123".to_string()),
            },
        ];

        let request = make_basic_request(messages);
        let result = transformer.transform_request(&request).unwrap();

        let msgs = result["messages"].as_array().unwrap();

        // Find the tool result message
        let tool_result_msg = msgs
            .iter()
            .find(|m| {
                m["content"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .any(|b| b["type"].as_str() == Some("tool_result"))
                    })
                    .unwrap_or(false)
            })
            .expect("Should have a tool_result message");

        assert_eq!(tool_result_msg["role"], "user");
        let content = tool_result_msg["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["content"], "Sunny, 72F");
    }

    #[test]
    fn test_transform_response_basic() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_01XFDUDYJgAACzvnptvVoYEL",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello! How can I help you?"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 25,
                "output_tokens": 10,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            }
        });

        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();

        assert_eq!(result.id, "msg_01XFDUDYJgAACzvnptvVoYEL");
        assert_eq!(result.object, "chat.completion");
        assert_eq!(result.choices.len(), 1);
        assert_eq!(
            result.choices[0].message.content.as_deref(),
            Some("Hello! How can I help you?")
        );
        assert_eq!(result.choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(result.usage.prompt_tokens, 25);
        assert_eq!(result.usage.completion_tokens, 10);
        assert_eq!(result.usage.total_tokens, 35);
    }

    #[test]
    fn test_transform_response_with_tool_use() {
        let transformer = make_transformer();
        let response = json!({
            "id": "msg_tools",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Let me check the weather."},
                {
                    "type": "tool_use",
                    "id": "toolu_01A09q90qw90lq917835lq9",
                    "name": "get_weather",
                    "input": {"location": "San Francisco"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 50,
                "output_tokens": 30
            }
        });

        let meta = make_meta();
        let result = transformer.transform_response(response, &meta).unwrap();

        assert_eq!(
            result.choices[0].message.content.as_deref(),
            Some("Let me check the weather.")
        );
        assert_eq!(
            result.choices[0].finish_reason.as_deref(),
            Some("tool_calls")
        );

        let tool_calls = result.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "get_weather");
    }

    #[test]
    fn test_stream_state_text_delta() {
        let mut state = ClaudeStreamState::new("claude-sonnet-4-20250514");

        // First send message_start to initialize
        let start_event = r#"{"type":"message_start","message":{"id":"msg_stream","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10}}}"#;
        state.process_event(start_event).unwrap();

        let event = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let result = state.process_event(event).unwrap();

        assert!(result.is_some());
        let chunk = result.unwrap();
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
        assert!(chunk.choices[0].delta.tool_calls.is_none());
    }

    #[test]
    fn test_stream_state_tool_call() {
        let mut state = ClaudeStreamState::new("claude-sonnet-4-20250514");

        // message_start
        let start_event = r#"{"type":"message_start","message":{"id":"msg_tool_stream","model":"claude-sonnet-4-20250514","usage":{"input_tokens":20}}}"#;
        let start_result = state.process_event(start_event).unwrap();
        assert!(start_result.is_some());
        let start_chunk = start_result.unwrap();
        assert_eq!(
            start_chunk.choices[0].delta.role.as_deref(),
            Some("assistant")
        );

        // content_block_start with tool_use
        let block_start = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"get_weather","input":{}}}"#;
        let result = state.process_event(block_start).unwrap();

        assert!(result.is_some());
        let chunk = result.unwrap();
        let tool_calls = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].index, Some(0));
        assert_eq!(tool_calls[0].function.name, "get_weather");
        assert_eq!(tool_calls[0].function.arguments, "");
        // Content must NOT be set for tool calls
        assert!(chunk.choices[0].delta.content.is_none());

        // input_json_delta
        let json_delta = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\": \"SF\"}"}}"#;
        let result = state.process_event(json_delta).unwrap();

        assert!(result.is_some());
        let chunk = result.unwrap();
        let tool_calls = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].index, Some(0));
        assert_eq!(tool_calls[0].function.arguments, r#"{"location": "SF"}"#);
        // Content must NOT be set for tool call deltas
        assert!(chunk.choices[0].delta.content.is_none());
    }

    #[test]
    fn test_stream_state_message_start_and_delta() {
        let mut state = ClaudeStreamState::new("claude-sonnet-4-20250514");

        // message_start
        let start_event = r#"{"type":"message_start","message":{"id":"msg_usage","model":"claude-sonnet-4-20250514","usage":{"input_tokens":42}}}"#;
        let result = state.process_event(start_event).unwrap();
        assert!(result.is_some());

        assert_eq!(state.response_id, "msg_usage");
        assert_eq!(state.input_tokens, 42);

        // message_delta with stop_reason and output usage
        let delta_event = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}"#;
        let result = state.process_event(delta_event).unwrap();

        assert!(result.is_some());
        let chunk = result.unwrap();
        assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("stop"));

        let usage = chunk.usage.as_ref().unwrap();
        assert_eq!(usage.prompt_tokens, 42);
        assert_eq!(usage.completion_tokens, 15);
        assert_eq!(usage.total_tokens, 57);

        // Verify final_usage
        let final_usage = state.final_usage();
        assert_eq!(final_usage.prompt_tokens, 42);
        assert_eq!(final_usage.completion_tokens, 15);
        assert_eq!(final_usage.total_tokens, 57);

        // Verify response_id
        assert_eq!(state.response_id(), "msg_usage");
    }
}
