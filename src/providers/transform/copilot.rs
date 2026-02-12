use crate::providers::ProviderError;
use crate::providers::transformer::{ProviderResponseMeta, ProviderTransformer, StreamState};
use crate::providers::types::*;

const SUPPORTED_MODELS: &[&str] = &[
    "gpt-4o",
    "gpt-4o-mini",
    "gpt-4",
    "gpt-4-turbo",
    "gpt-3.5-turbo",
    "o1-mini",
    "o1-preview",
    "o3-mini",
    "claude-3.5-sonnet",
];

pub struct CopilotTransformer;

impl CopilotTransformer {
    pub fn new() -> Self {
        Self
    }

    fn role_to_str(role: &MessageRole) -> &'static str {
        match role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }

    fn serialize_message(msg: &ChatMessage) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "role".into(),
            serde_json::Value::String(Self::role_to_str(&msg.role).into()),
        );

        match &msg.content {
            Some(MessageContent::Text(text)) => {
                obj.insert("content".into(), serde_json::Value::String(text.clone()));
            }
            Some(MessageContent::Parts(parts)) => {
                let parts_json: Vec<serde_json::Value> = parts
                    .iter()
                    .map(|p| serde_json::to_value(p).unwrap_or(serde_json::Value::Null))
                    .collect();
                obj.insert("content".into(), serde_json::Value::Array(parts_json));
            }
            None => {
                obj.insert("content".into(), serde_json::Value::Null);
            }
        }

        if let Some(name) = &msg.name {
            obj.insert("name".into(), serde_json::Value::String(name.clone()));
        }

        if let Some(tool_call_id) = &msg.tool_call_id {
            obj.insert(
                "tool_call_id".into(),
                serde_json::Value::String(tool_call_id.clone()),
            );
        }

        if let Some(tool_calls) = &msg.tool_calls {
            let tc_json: Vec<serde_json::Value> = tool_calls
                .iter()
                .map(|tc| {
                    let mut tc_obj = serde_json::Map::new();
                    if let Some(idx) = tc.index {
                        tc_obj.insert("index".into(), serde_json::Value::Number(idx.into()));
                    }
                    tc_obj.insert("id".into(), serde_json::Value::String(tc.id.clone()));
                    tc_obj.insert("type".into(), serde_json::Value::String(tc.r#type.clone()));
                    let mut fn_obj = serde_json::Map::new();
                    fn_obj.insert(
                        "name".into(),
                        serde_json::Value::String(tc.function.name.clone()),
                    );
                    fn_obj.insert(
                        "arguments".into(),
                        serde_json::Value::String(tc.function.arguments.clone()),
                    );
                    tc_obj.insert("function".into(), serde_json::Value::Object(fn_obj));
                    serde_json::Value::Object(tc_obj)
                })
                .collect();
            obj.insert("tool_calls".into(), serde_json::Value::Array(tc_json));
        }

        serde_json::Value::Object(obj)
    }

    fn parse_tool_calls(val: &serde_json::Value) -> Option<Vec<ToolCall>> {
        val.as_array().map(|arr| {
            arr.iter()
                .map(|tc| ToolCall {
                    index: tc.get("index").and_then(|v| v.as_u64()).map(|v| v as u32),
                    id: tc
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    r#type: tc
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("function")
                        .to_string(),
                    function: FunctionCall {
                        name: tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        arguments: tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    },
                })
                .collect()
        })
    }

    fn parse_usage(val: &serde_json::Value) -> Usage {
        let prompt = val
            .get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let completion = val
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let total = val
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(prompt as u64 + completion as u64) as u32;

        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: total,
            prompt_tokens_details: val
                .get("prompt_tokens_details")
                .and_then(|d| serde_json::from_value::<UsageTokenDetails>(d.clone()).ok()),
            completion_tokens_details: val
                .get("completion_tokens_details")
                .and_then(|d| serde_json::from_value::<UsageTokenDetails>(d.clone()).ok()),
        }
    }
}

impl ProviderTransformer for CopilotTransformer {
    fn transform_request(&self, request: &ChatRequest) -> Result<serde_json::Value, ProviderError> {
        let mut body = serde_json::Map::new();

        body.insert(
            "model".into(),
            serde_json::Value::String(request.model.clone()),
        );

        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(Self::serialize_message)
            .collect();
        body.insert("messages".into(), serde_json::Value::Array(messages));

        if let Some(temp) = request.temperature {
            body.insert("temperature".into(), serde_json::json!(temp));
        }

        if let Some(max_tokens) = request.max_tokens {
            body.insert(
                "max_tokens".into(),
                serde_json::Value::Number(max_tokens.into()),
            );
        }

        if request.stream {
            body.insert("stream".into(), serde_json::Value::Bool(true));
        }

        if let Some(top_p) = request.top_p {
            body.insert("top_p".into(), serde_json::json!(top_p));
        }

        if let Some(stop) = &request.stop {
            body.insert(
                "stop".into(),
                serde_json::to_value(stop).map_err(|e| {
                    ProviderError::InvalidRequest(format!("failed to serialize stop: {e}"))
                })?,
            );
        }

        if let Some(tools) = &request.tools {
            body.insert(
                "tools".into(),
                serde_json::to_value(tools).map_err(|e| {
                    ProviderError::InvalidRequest(format!("failed to serialize tools: {e}"))
                })?,
            );
        }

        if let Some(tool_choice) = &request.tool_choice {
            body.insert(
                "tool_choice".into(),
                serde_json::to_value(tool_choice).map_err(|e| {
                    ProviderError::InvalidRequest(format!("failed to serialize tool_choice: {e}"))
                })?,
            );
        }

        Ok(serde_json::Value::Object(body))
    }

    fn transform_response(
        &self,
        response: serde_json::Value,
        _meta: &ProviderResponseMeta,
    ) -> Result<ChatResponse, ProviderError> {
        let id = response
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let object = response
            .get("object")
            .and_then(|v| v.as_str())
            .unwrap_or("chat.completion")
            .to_string();

        let created = response
            .get("created")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let model = response
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let choices = response
            .get("choices")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|c| {
                        let message = c.get("message").unwrap_or(&serde_json::Value::Null);
                        let role_str = message
                            .get("role")
                            .and_then(|v| v.as_str())
                            .unwrap_or("assistant");
                        let content = message
                            .get("content")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let tool_calls = message
                            .get("tool_calls")
                            .and_then(|tc| Self::parse_tool_calls(tc));

                        Choice {
                            index: c.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                            message: ResponseMessage {
                                role: role_str.to_string(),
                                content,
                                reasoning_content: None,
                                tool_calls,
                            },
                            finish_reason: c
                                .get("finish_reason")
                                .and_then(|v| v.as_str())
                                .map(|r| self.map_finish_reason(r).to_string()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let usage = response
            .get("usage")
            .map(Self::parse_usage)
            .unwrap_or_default();

        Ok(ChatResponse {
            id,
            object,
            created,
            model,
            choices,
            usage,
        })
    }

    fn new_stream_state(&self, model: &str) -> Box<dyn StreamState> {
        Box::new(CopilotStreamState {
            response_id: String::new(),
            model: model.to_string(),
            usage: Usage::default(),
        })
    }

    fn provider_id(&self) -> &str {
        "copilot"
    }

    fn provider_name(&self) -> &str {
        "GitHub Copilot"
    }

    fn supports_model(&self, model: &str) -> bool {
        SUPPORTED_MODELS.contains(&model)
    }

    fn supported_models(&self) -> Vec<String> {
        SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect()
    }

    fn map_finish_reason(&self, reason: &str) -> &'static str {
        match reason {
            "stop" => "stop",
            "length" => "length",
            "tool_calls" => "tool_calls",
            "content_filter" => "content_filter",
            _ => "stop",
        }
    }
}

pub struct CopilotStreamState {
    response_id: String,
    model: String,
    usage: Usage,
}

impl StreamState for CopilotStreamState {
    fn process_event(&mut self, data: &str) -> Result<Option<ChatChunk>, ProviderError> {
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Ok(None);
        }

        let v: serde_json::Value = serde_json::from_str(data).map_err(|e| {
            ProviderError::ResponseParsing(format!("failed to parse stream chunk: {e}"))
        })?;

        let id = v
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if !id.is_empty() {
            self.response_id = id.clone();
        }

        let model = v
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.model)
            .to_string();
        self.model = model.clone();

        let created = v.get("created").and_then(|v| v.as_i64()).unwrap_or(0);

        // Extract usage if present
        if let Some(usage_val) = v.get("usage") {
            if !usage_val.is_null() {
                self.usage = CopilotTransformer::parse_usage(usage_val);
            }
        }

        let choices = v
            .get("choices")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|c| {
                        let delta = c.get("delta").unwrap_or(&serde_json::Value::Null);
                        let role = delta
                            .get("role")
                            .and_then(|v| v.as_str())
                            .map(|r| r.to_string());
                        let content = delta
                            .get("content")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let reasoning_content = delta
                            .get("reasoning_content")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let tool_calls = delta
                            .get("tool_calls")
                            .and_then(|tc| CopilotTransformer::parse_tool_calls(tc));

                        ChunkChoice {
                            index: c.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                            delta: Delta {
                                role,
                                content,
                                reasoning_content,
                                tool_calls,
                            },
                            finish_reason: c
                                .get("finish_reason")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Some(ChatChunk {
            id,
            object: "chat.completion.chunk".to_string(),
            created,
            model,
            choices,
            usage: None,
        }))
    }

    fn final_usage(&self) -> Usage {
        self.usage.clone()
    }

    fn response_id(&self) -> &str {
        &self.response_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_transformer() -> CopilotTransformer {
        CopilotTransformer::new()
    }

    #[test]
    fn test_transform_request_basic() {
        let transformer = make_transformer();
        let request = ChatRequest {
            model: "gpt-4o".to_string(),
            messages: vec![
                ChatMessage {
                    role: MessageRole::System,
                    content: Some(MessageContent::Text("You are helpful.".to_string())),
                    name: None,
                    tool_call_id: None,
                    tool_calls: None,
                },
                ChatMessage {
                    role: MessageRole::User,
                    content: Some(MessageContent::Text("Hello".to_string())),
                    name: None,
                    tool_call_id: None,
                    tool_calls: None,
                },
            ],
            temperature: Some(0.7),
            max_tokens: Some(1024),
            stream: true,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        };

        let result = transformer.transform_request(&request).unwrap();

        assert_eq!(result["model"], "gpt-4o");
        let temp = result["temperature"].as_f64().unwrap();
        assert!(
            (temp - 0.7).abs() < 0.001,
            "temperature should be ~0.7, got {temp}"
        );
        assert_eq!(result["max_tokens"], 1024);
        assert_eq!(result["stream"], true);

        let messages = result["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are helpful.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello");

        // Optional fields not present when None
        assert!(result.get("top_p").is_none());
        assert!(result.get("stop").is_none());
        assert!(result.get("tools").is_none());
        assert!(result.get("tool_choice").is_none());
    }

    #[test]
    fn test_transform_response_basic() {
        let transformer = make_transformer();
        let response = serde_json::json!({
            "id": "chatcmpl-abc123",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello! How can I help?"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 10,
                "total_tokens": 30
            }
        });

        let meta = ProviderResponseMeta::default();
        let result = transformer.transform_response(response, &meta).unwrap();

        assert_eq!(result.id, "chatcmpl-abc123");
        assert_eq!(result.object, "chat.completion");
        assert_eq!(result.created, 1700000000);
        assert_eq!(result.model, "gpt-4o");
        assert_eq!(result.choices.len(), 1);
        assert_eq!(
            result.choices[0].message.content.as_deref(),
            Some("Hello! How can I help?")
        );
        assert_eq!(result.choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(result.usage.prompt_tokens, 20);
        assert_eq!(result.usage.completion_tokens, 10);
        assert_eq!(result.usage.total_tokens, 30);
    }

    #[test]
    fn test_stream_state_text() {
        let transformer = make_transformer();
        let mut state = transformer.new_stream_state("gpt-4o");

        let chunk_data = r#"{"id":"chatcmpl-xyz","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}],"usage":null}"#;

        let result = state.process_event(chunk_data).unwrap();
        assert!(result.is_some());

        let chunk = result.unwrap();
        assert_eq!(chunk.id, "chatcmpl-xyz");
        assert_eq!(chunk.model, "gpt-4o");
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
        assert_eq!(chunk.choices[0].delta.role.as_deref(), Some("assistant"));
        assert!(chunk.choices[0].finish_reason.is_none());

        // [DONE] returns None
        let done = state.process_event("[DONE]").unwrap();
        assert!(done.is_none());

        assert_eq!(state.response_id(), "chatcmpl-xyz");
    }

    #[test]
    fn test_stream_state_with_usage() {
        let transformer = make_transformer();
        let mut state = transformer.new_stream_state("gpt-4o");

        // First chunk with content
        let chunk1 = r#"{"id":"chatcmpl-u1","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}],"usage":null}"#;
        state.process_event(chunk1).unwrap();

        // Final chunk with usage
        let chunk2 = r#"{"id":"chatcmpl-u1","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[],"usage":{"prompt_tokens":15,"completion_tokens":5,"total_tokens":20}}"#;
        state.process_event(chunk2).unwrap();

        let usage = state.final_usage();
        assert_eq!(usage.prompt_tokens, 15);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 20);
    }
}
