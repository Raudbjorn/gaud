use crate::providers::transform::util;
use crate::providers::transformer::{ProviderResponseMeta, ProviderTransformer, StreamState};
use crate::providers::types::*;
use crate::providers::ProviderError;
use serde_json::{json, Value};
use uuid::Uuid;

const SUPPORTED_MODELS: &[&str] = &[
    "gemini-2.5-flash",
    "gemini-2.5-pro",
    "gemini-2.0-flash",
    "gemini-1.5-flash",
    "gemini-1.5-pro",
];

pub struct GeminiTransformer;

impl GeminiTransformer {
    pub fn new() -> Self {
        Self
    }

    /// Convert an OpenAI-format message into a Gemini `contents` entry.
    fn convert_message(msg: &ChatMessage) -> Option<Value> {
        match msg.role {
            // System messages are handled separately via system_instruction.
            MessageRole::System => None,

            MessageRole::User => {
                let parts = Self::build_user_parts(msg);
                Some(json!({ "role": "user", "parts": Self::ensure_non_empty_parts(parts) }))
            }

            MessageRole::Assistant => {
                let mut parts: Vec<Value> = Vec::new();

                // Text content
                if let Some(text) = Self::extract_text(msg) {
                    if !text.is_empty() {
                        parts.push(json!({ "text": text }));
                    }
                }

                // Tool calls become functionCall parts
                if let Some(ref tool_calls) = msg.tool_calls {
                    for tc in tool_calls {
                        let args: Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                        parts.push(json!({
                            "functionCall": {
                                "name": tc.function.name,
                                "args": args
                            }
                        }));
                    }
                }

                Some(json!({ "role": "model", "parts": Self::ensure_non_empty_parts(parts) }))
            }

            MessageRole::Tool => {
                // Tool result messages become user role with functionResponse parts.
                let name = msg
                    .name
                    .clone()
                    .or_else(|| msg.tool_call_id.clone())
                    .unwrap_or_else(|| "function".to_string());
                let text = Self::extract_text(msg).unwrap_or_default();
                let parts = vec![json!({
                    "functionResponse": {
                        "name": name,
                        "response": { "result": text }
                    }
                })];
                Some(json!({ "role": "user", "parts": parts }))
            }
        }
    }

    /// Build parts for a user message, handling both plain text and multimodal content.
    fn build_user_parts(msg: &ChatMessage) -> Vec<Value> {
        match &msg.content {
            Some(MessageContent::Text(text)) => {
                vec![json!({ "text": text })]
            }
            Some(MessageContent::Parts(parts)) => {
                let mut out = Vec::new();
                for part in parts {
                    match part {
                        ContentPart::Text { text } => {
                            out.push(json!({ "text": text }));
                        }
                        ContentPart::ImageUrl { image_url } => {
                            let (source_type, media_type, data) =
                                util::parse_image_url(&image_url.url);
                            match source_type.as_str() {
                                "base64" => {
                                    out.push(json!({
                                        "inlineData": {
                                            "mimeType": media_type,
                                            "data": data
                                        }
                                    }));
                                }
                                _ => {
                                    out.push(json!({
                                        "fileData": {
                                            "mimeType": media_type,
                                            "fileUri": data
                                        }
                                    }));
                                }
                            }
                        }
                    }
                }
                out
            }
            None => Vec::new(),
        }
    }

    /// Extract plain text from a message's content field.
    fn extract_text(msg: &ChatMessage) -> Option<String> {
        match &msg.content {
            Some(MessageContent::Text(t)) => Some(t.clone()),
            Some(MessageContent::Parts(parts)) => {
                let texts: Vec<&str> = parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                if texts.is_empty() {
                    None
                } else {
                    Some(texts.join("\n"))
                }
            }
            None => None,
        }
    }

    /// Ensure the parts array is never empty; inject a placeholder if needed.
    fn ensure_non_empty_parts(parts: Vec<Value>) -> Vec<Value> {
        if parts.is_empty() {
            vec![json!({ "text": " " })]
        } else {
            parts
        }
    }
}

impl ProviderTransformer for GeminiTransformer {
    fn transform_request(&self, request: &ChatRequest) -> Result<Value, ProviderError> {
        let mut body = json!({});

        // System instruction from system messages
        let system_text = util::concatenate_system_messages(&request.messages);
        if let Some(text) = system_text {
            body["system_instruction"] = json!({ "parts": [{ "text": text }] });
        }

        // Contents from non-system messages
        let non_system = util::filter_system_messages(&request.messages);
        let contents: Vec<Value> = non_system
            .iter()
            .filter_map(|msg| Self::convert_message(msg))
            .collect();
        body["contents"] = json!(contents);

        // Generation config
        let mut gen_config = json!({});
        if let Some(temp) = request.temperature {
            gen_config["temperature"] = json!(temp);
        }
        if let Some(top_p) = request.top_p {
            gen_config["topP"] = json!(top_p);
        }
        if let Some(max_tokens) = request.max_tokens {
            gen_config["maxOutputTokens"] = json!(max_tokens);
        }
        if let Some(ref stop) = request.stop {
            if let Some(sequences) = util::normalize_stop_sequences(&Some(stop.clone())) {
                gen_config["stopSequences"] = json!(sequences);
            }
        }
        if gen_config != json!({}) {
            body["generationConfig"] = gen_config;
        }

        // Tools
        if let Some(ref tools) = request.tools {
            if !tools.is_empty() {
                let gemini_tools = util::convert_tools_to_gemini(tools);
                body["tools"] = json!(gemini_tools);
            }
        }

        // Tool choice
        if let Some(tc) = util::convert_tool_choice_gemini(&request.tool_choice) {
            body["tool_config"] = tc;
        }

        Ok(body)
    }

    fn transform_response(
        &self,
        response: Value,
        meta: &ProviderResponseMeta,
    ) -> Result<ChatResponse, ProviderError> {
        let candidates = response["candidates"]
            .as_array()
            .ok_or_else(|| ProviderError::ResponseParsing("Missing candidates array".into()))?;

        let mut choices = Vec::new();
        for (i, candidate) in candidates.iter().enumerate() {
            let parts = candidate["content"]["parts"].as_array();
            let finish_reason = candidate["finishReason"]
                .as_str()
                .map(|r| self.map_finish_reason(r).to_string());

            let mut text_parts: Vec<String> = Vec::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut tool_idx: u32 = 0;

            if let Some(parts) = parts {
                for part in parts {
                    if let Some(text) = part["text"].as_str() {
                        text_parts.push(text.to_string());
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc["name"].as_str().unwrap_or("").to_string();
                        let args = &fc["args"];
                        let arguments = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
                        tool_calls.push(ToolCall {
                            index: Some(tool_idx),
                            id: format!("call_{}", Uuid::new_v4()),
                            r#type: "function".to_string(),
                            function: FunctionCall {
                                name,
                                arguments,
                            },
                        });
                        tool_idx += 1;
                    }
                }
            }

            let content = if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join(""))
            };

            let tc = if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            };

            choices.push(Choice {
                index: i as u32,
                message: ResponseMessage {
                    role: "assistant".to_string(),
                    content,
                    reasoning_content: None,
                    tool_calls: tc,
                },
                finish_reason,
            });
        }

        // Usage
        let usage_meta = &response["usageMetadata"];
        let prompt_tokens = usage_meta["promptTokenCount"].as_u64().unwrap_or(0) as u32;
        let completion_tokens = usage_meta["candidatesTokenCount"].as_u64().unwrap_or(0) as u32;
        let total_tokens = usage_meta["totalTokenCount"]
            .as_u64()
            .unwrap_or((prompt_tokens + completion_tokens) as u64) as u32;

        let usage = Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };

        Ok(ChatResponse {
            id: format!("chatcmpl-{}", Uuid::new_v4()),
            object: "chat.completion".to_string(),
            created: meta.created,
            model: meta.model.clone(),
            choices,
            usage,
        })
    }

    fn new_stream_state(&self, model: &str) -> Box<dyn StreamState> {
        Box::new(GeminiStreamState::new(model.to_string()))
    }

    fn provider_id(&self) -> &str {
        "gemini"
    }

    fn provider_name(&self) -> &str {
        "Google Gemini"
    }

    fn supports_model(&self, model: &str) -> bool {
        SUPPORTED_MODELS.contains(&model)
    }

    fn supported_models(&self) -> Vec<String> {
        SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect()
    }

    fn map_finish_reason(&self, reason: &str) -> &'static str {
        util::map_finish_reason_to_openai(reason)
    }
}

pub struct GeminiStreamState {
    response_id: String,
    model: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    tool_index: i32,
}

impl GeminiStreamState {
    pub fn new(model: String) -> Self {
        Self {
            response_id: format!("chatcmpl-{}", Uuid::new_v4()),
            model,
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_index: -1,
        }
    }
}

impl StreamState for GeminiStreamState {
    fn process_event(&mut self, data: &str) -> Result<Option<ChatChunk>, ProviderError> {
        // Gemini streams SSE with JSON data lines
        let parsed: Value = serde_json::from_str(data)
            .map_err(|e| ProviderError::ResponseParsing(format!("Invalid JSON in stream: {e}")))?;

        // Extract usage metadata if present
        if let Some(usage) = parsed.get("usageMetadata") {
            if let Some(pt) = usage["promptTokenCount"].as_u64() {
                self.prompt_tokens = pt as u32;
            }
            if let Some(ct) = usage["candidatesTokenCount"].as_u64() {
                self.completion_tokens = ct as u32;
            }
        }

        // Process candidates
        let candidates = match parsed["candidates"].as_array() {
            Some(c) => c,
            None => return Ok(None),
        };

        let candidate = match candidates.first() {
            Some(c) => c,
            None => return Ok(None),
        };

        let finish_reason = candidate["finishReason"]
            .as_str()
            .map(|r| util::map_finish_reason_to_openai(r).to_string());

        let parts = match candidate["content"]["parts"].as_array() {
            Some(p) => p,
            None => {
                // No parts but possibly a finish reason
                if finish_reason.is_some() {
                    return Ok(Some(ChatChunk {
                        id: self.response_id.clone(),
                        object: "chat.completion.chunk".to_string(),
                        created: 0,
                        model: self.model.clone(),
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: Delta {
                                role: None,
                                content: None,
                                reasoning_content: None,
                                tool_calls: None,
                            },
                            finish_reason,
                        }],
                        usage: None,
                    }));
                }
                return Ok(None);
            }
        };

        // Collect text parts and function call parts
        let mut text_content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for part in parts {
            if let Some(text) = part["text"].as_str() {
                text_content.push_str(text);
            }

            if let Some(fc) = part.get("functionCall") {
                self.tool_index += 1;
                let name = fc["name"].as_str().unwrap_or("").to_string();
                let args = &fc["args"];
                let arguments = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());

                tool_calls.push(ToolCall {
                    index: Some(self.tool_index as u32),
                    id: format!("call_{}", Uuid::new_v4()),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name,
                        arguments,
                    },
                });
            }
        }

        let delta_content = if text_content.is_empty() {
            None
        } else {
            Some(text_content)
        };

        let delta_tool_calls = if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        };

        // Only emit a chunk if there is content, tool calls, or a finish reason
        if delta_content.is_none() && delta_tool_calls.is_none() && finish_reason.is_none() {
            return Ok(None);
        }

        Ok(Some(ChatChunk {
            id: self.response_id.clone(),
            object: "chat.completion.chunk".to_string(),
            created: 0,
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: Some("assistant".to_string()),
                    content: delta_content,
                    reasoning_content: None,
                    tool_calls: delta_tool_calls,
                },
                finish_reason,
            }],
            usage: None,
        }))
    }

    fn final_usage(&self) -> Usage {
        Usage {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            total_tokens: self.prompt_tokens + self.completion_tokens,
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

    fn make_transformer() -> GeminiTransformer {
        GeminiTransformer::new()
    }

    fn simple_request(messages: Vec<ChatMessage>) -> ChatRequest {
        ChatRequest {
            model: "gemini-2.0-flash".to_string(),
            messages,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stop: None,
            stream: false,
            tools: None,
            tool_choice: None,
            stream_options: None,
        }
    }

    #[test]
    fn test_transform_request_basic() {
        let t = make_transformer();
        let req = simple_request(vec![ChatMessage {
            role: MessageRole::User,
            content: Some(MessageContent::Text("Hello".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }]);

        let result = t.transform_request(&req).unwrap();
        let contents = result["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Hello");
        assert!(result.get("system_instruction").is_none());
    }

    #[test]
    fn test_transform_request_with_system() {
        let t = make_transformer();
        let req = simple_request(vec![
            ChatMessage {
                role: MessageRole::System,
                content: Some(MessageContent::Text("You are helpful".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hi".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ]);

        let result = t.transform_request(&req).unwrap();

        // System instruction should be extracted
        let sys = &result["system_instruction"];
        assert_eq!(sys["parts"][0]["text"], "You are helpful");

        // Contents should only have the user message
        let contents = result["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
    }

    #[test]
    fn test_transform_request_with_tools() {
        let t = make_transformer();
        let mut req = simple_request(vec![ChatMessage {
            role: MessageRole::User,
            content: Some(MessageContent::Text("What is the weather?".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }]);
        req.tools = Some(vec![Tool {
            r#type: "function".to_string(),
            function: FunctionDef {
                name: "get_weather".to_string(),
                description: Some("Get weather for a location".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "location": { "type": "string" }
                    },
                    "required": ["location"]
                })),
            },
        }]);

        let result = t.transform_request(&req).unwrap();
        assert!(result.get("tools").is_some());
        let tools = result["tools"].as_array().unwrap();
        assert!(!tools.is_empty());
    }

    #[test]
    fn test_transform_request_empty_parts_injection() {
        let t = make_transformer();
        // Assistant message with no content and no tool_calls should get placeholder
        let req = simple_request(vec![
            ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hi".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: MessageRole::Assistant,
                content: None,
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Continue".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ]);

        let result = t.transform_request(&req).unwrap();
        let contents = result["contents"].as_array().unwrap();
        // The assistant message (index 1) should have a placeholder part
        let model_msg = &contents[1];
        assert_eq!(model_msg["role"], "model");
        let parts = model_msg["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["text"], " ");
    }

    #[test]
    fn test_transform_response_basic() {
        let t = make_transformer();
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{ "text": "Hello there!" }],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "totalTokenCount": 15
            }
        });

        let meta = ProviderResponseMeta {
            model: "gemini-2.0-flash".to_string(),
            created: 1700000000,
            ..Default::default()
        };

        let result = t.transform_response(response, &meta).unwrap();
        assert_eq!(result.object, "chat.completion");
        assert_eq!(result.model, "gemini-2.0-flash");
        assert_eq!(result.choices.len(), 1);
        assert_eq!(
            result.choices[0].message.content.as_deref(),
            Some("Hello there!")
        );
        assert_eq!(result.choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(result.usage.prompt_tokens, 10);
        assert_eq!(result.usage.completion_tokens, 5);
        assert_eq!(result.usage.total_tokens, 15);
    }

    #[test]
    fn test_transform_response_with_function_call() {
        let t = make_transformer();
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "get_weather",
                            "args": { "location": "London" }
                        }
                    }],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "usageMetadata": {
                "promptTokenCount": 20,
                "candidatesTokenCount": 10,
                "totalTokenCount": 30
            }
        });

        let meta = ProviderResponseMeta {
            model: "gemini-2.0-flash".to_string(),
            created: 1700000000,
            ..Default::default()
        };

        let result = t.transform_response(response, &meta).unwrap();
        assert!(result.choices[0].message.content.is_none());
        let tool_calls = result.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "get_weather");
        assert!(tool_calls[0].id.starts_with("call_"));
        assert_eq!(tool_calls[0].r#type, "function");

        let args: Value = serde_json::from_str(&tool_calls[0].function.arguments).unwrap();
        assert_eq!(args["location"], "London");
    }

    #[test]
    fn test_stream_state_text() {
        let mut state = GeminiStreamState::new("gemini-2.0-flash".to_string());

        let data = json!({
            "candidates": [{
                "content": {
                    "parts": [{ "text": "Hello" }],
                    "role": "model"
                },
                "index": 0
            }]
        });

        let chunk = state
            .process_event(&serde_json::to_string(&data).unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(chunk.object, "chat.completion.chunk");
        assert_eq!(chunk.model, "gemini-2.0-flash");
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(
            chunk.choices[0].delta.content.as_deref(),
            Some("Hello")
        );
        assert!(chunk.choices[0].delta.tool_calls.is_none());
        assert!(chunk.choices[0].finish_reason.is_none());
    }

    #[test]
    fn test_stream_state_function_call() {
        let mut state = GeminiStreamState::new("gemini-2.0-flash".to_string());

        let data = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "get_weather",
                            "args": { "location": "Paris" }
                        }
                    }],
                    "role": "model"
                },
                "index": 0
            }]
        });

        let chunk = state
            .process_event(&serde_json::to_string(&data).unwrap())
            .unwrap()
            .unwrap();

        let tool_calls = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].index, Some(0));
        assert!(tool_calls[0].id.starts_with("call_"));
        assert_eq!(tool_calls[0].r#type, "function");
        assert_eq!(tool_calls[0].function.name, "get_weather");

        let args: Value = serde_json::from_str(&tool_calls[0].function.arguments).unwrap();
        assert_eq!(args["location"], "Paris");

        // Verify tool_index incremented
        assert_eq!(state.tool_index, 0);

        // Second function call should increment index
        let data2 = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "get_time",
                            "args": { "timezone": "CET" }
                        }
                    }],
                    "role": "model"
                },
                "index": 0
            }]
        });

        let chunk2 = state
            .process_event(&serde_json::to_string(&data2).unwrap())
            .unwrap()
            .unwrap();

        let tool_calls2 = chunk2.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls2[0].index, Some(1));
        assert_eq!(state.tool_index, 1);
    }

    #[test]
    fn test_stream_state_usage_tracking() {
        let mut state = GeminiStreamState::new("gemini-2.0-flash".to_string());

        let data = json!({
            "candidates": [{
                "content": {
                    "parts": [{ "text": "Done" }],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "usageMetadata": {
                "promptTokenCount": 42,
                "candidatesTokenCount": 17,
                "totalTokenCount": 59
            }
        });

        let chunk = state
            .process_event(&serde_json::to_string(&data).unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(
            chunk.choices[0].finish_reason.as_deref(),
            Some("stop")
        );

        let usage = state.final_usage();
        assert_eq!(usage.prompt_tokens, 42);
        assert_eq!(usage.completion_tokens, 17);
        assert_eq!(usage.total_tokens, 59);

        // Verify response_id is stable
        assert!(state.response_id().starts_with("chatcmpl-"));
    }
}
