//! Gemini (Google) Provider
//!
//! Converts OpenAI-format requests into Google Generative AI content format,
//! sends them to the Gemini API, and converts the response back to OpenAI
//! format.

use std::pin::Pin;

use futures::stream::{self, StreamExt};
use futures::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::providers::types::*;
use crate::providers::{LlmProvider, ProviderError, TokenStorage};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const DEFAULT_MAX_TOKENS: u32 = 8_192;

const SUPPORTED_MODELS: &[&str] = &[
    "gemini-2.5-flash",
    "gemini-2.5-pro",
    "gemini-2.0-flash",
];

// ---------------------------------------------------------------------------
// Gemini API types (request)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum GeminiPart {
    #[serde(rename = "text")]
    Text(String),
    #[serde(rename = "inlineData")]
    InlineData {
        mime_type: String,
        data: String,
    },
    #[serde(rename = "functionCall")]
    FunctionCall {
        name: String,
        args: serde_json::Value,
    },
    #[serde(rename = "functionResponse")]
    FunctionResponse {
        name: String,
        response: serde_json::Value,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTool {
    function_declarations: Vec<GeminiFunctionDecl>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionDecl {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Gemini API types (response)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsageMetadata>,
    #[allow(dead_code)]
    model_version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
}

// ---------------------------------------------------------------------------
// Gemini stream response (array of GenerateContentResponse)
// ---------------------------------------------------------------------------

// The streaming endpoint returns newline-delimited JSON objects (not SSE).
// Each object has the same structure as GenerateContentResponse.

// ---------------------------------------------------------------------------
// Gemini Provider
// ---------------------------------------------------------------------------

/// LLM provider that communicates with the Google Gemini API.
pub struct GeminiProvider<T: TokenStorage> {
    http: Client,
    tokens: std::sync::Arc<T>,
}

impl<T: TokenStorage + 'static> GeminiProvider<T> {
    /// Create a new Gemini provider backed by the given token storage.
    pub fn new(tokens: std::sync::Arc<T>) -> Self {
        Self {
            http: Client::new(),
            tokens,
        }
    }

    /// Retrieve an access token or return an error.
    async fn get_token(&self) -> Result<String, ProviderError> {
        self.tokens
            .get_access_token("gemini")
            .await?
            .ok_or_else(|| ProviderError::NoToken("gemini".into()))
    }

    // -- conversion helpers --------------------------------------------------

    fn convert_request(request: &ChatRequest) -> GeminiRequest {
        let mut system_instruction: Option<GeminiContent> = None;
        let mut contents: Vec<GeminiContent> = Vec::new();

        for msg in &request.messages {
            match msg.role {
                MessageRole::System => {
                    if let Some(content) = &msg.content {
                        system_instruction = Some(GeminiContent {
                            role: None,
                            parts: vec![GeminiPart::Text(content.as_text().to_string())],
                        });
                    }
                }
                MessageRole::Tool => {
                    // Tool responses go as function_response parts
                    let content_text = msg
                        .content
                        .as_ref()
                        .map(|c| c.as_text().to_string())
                        .unwrap_or_default();
                    let name = msg.name.clone().unwrap_or_default();
                    let response_value: serde_json::Value =
                        serde_json::from_str(&content_text).unwrap_or(serde_json::json!({
                            "result": content_text
                        }));
                    contents.push(GeminiContent {
                        role: Some("function".into()),
                        parts: vec![GeminiPart::FunctionResponse {
                            name,
                            response: response_value,
                        }],
                    });
                }
                _ => {
                    let role = match msg.role {
                        MessageRole::User => "user",
                        MessageRole::Assistant => "model",
                        _ => "user",
                    };

                    let parts = Self::convert_message_parts(msg);

                    contents.push(GeminiContent {
                        role: Some(role.into()),
                        parts,
                    });
                }
            }
        }

        let stop_sequences = request.stop.as_ref().map(|s| match s {
            StopSequence::Single(s) => vec![s.clone()],
            StopSequence::Multiple(v) => v.clone(),
        });

        let generation_config = if request.temperature.is_some()
            || request.top_p.is_some()
            || request.max_tokens.is_some()
            || stop_sequences.is_some()
        {
            Some(GeminiGenerationConfig {
                temperature: request.temperature,
                top_p: request.top_p,
                max_output_tokens: request.max_tokens.or(Some(DEFAULT_MAX_TOKENS)),
                stop_sequences,
            })
        } else {
            Some(GeminiGenerationConfig {
                temperature: None,
                top_p: None,
                max_output_tokens: Some(DEFAULT_MAX_TOKENS),
                stop_sequences: None,
            })
        };

        // Convert OpenAI tools to Gemini format.
        let tools = request.tools.as_ref().map(|openai_tools| {
            let decls: Vec<GeminiFunctionDecl> = openai_tools
                .iter()
                .map(|t| GeminiFunctionDecl {
                    name: t.function.name.clone(),
                    description: t.function.description.clone(),
                    parameters: t.function.parameters.clone(),
                })
                .collect();
            vec![GeminiTool {
                function_declarations: decls,
            }]
        });

        GeminiRequest {
            contents,
            system_instruction,
            generation_config,
            tools,
        }
    }

    fn convert_message_parts(msg: &ChatMessage) -> Vec<GeminiPart> {
        let Some(content) = &msg.content else {
            // If the assistant message has tool_calls but no text content,
            // convert them.
            if let Some(tool_calls) = &msg.tool_calls {
                return tool_calls
                    .iter()
                    .map(|tc| {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                        GeminiPart::FunctionCall {
                            name: tc.function.name.clone(),
                            args,
                        }
                    })
                    .collect();
            }
            return vec![];
        };

        let mut parts: Vec<GeminiPart> = match content {
            MessageContent::Text(text) => vec![GeminiPart::Text(text.clone())],
            MessageContent::Parts(content_parts) => content_parts
                .iter()
                .map(|p| match p {
                    ContentPart::Text { text } => GeminiPart::Text(text.clone()),
                    ContentPart::ImageUrl { image_url } => {
                        if image_url.url.starts_with("data:") {
                            let url_parts: Vec<&str> =
                                image_url.url.splitn(2, ',').collect();
                            let mime = url_parts
                                .first()
                                .unwrap_or(&"")
                                .trim_start_matches("data:")
                                .split(';')
                                .next()
                                .unwrap_or("image/png")
                                .to_string();
                            let data = url_parts.get(1).unwrap_or(&"").to_string();
                            GeminiPart::InlineData {
                                mime_type: mime,
                                data,
                            }
                        } else {
                            // Non-data URI: pass as text reference (Gemini
                            // doesn't support external URLs directly in the
                            // same way; for now we embed the URL as text).
                            GeminiPart::Text(format!("[image: {}]", image_url.url))
                        }
                    }
                })
                .collect(),
        };

        // Append tool_calls as function_call parts.
        if let Some(tool_calls) = &msg.tool_calls {
            for tc in tool_calls {
                let args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                parts.push(GeminiPart::FunctionCall {
                    name: tc.function.name.clone(),
                    args,
                });
            }
        }

        parts
    }

    fn convert_response(model: &str, api_resp: GeminiResponse) -> ChatResponse {
        let mut text_parts = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut finish_reason: Option<String> = None;

        if let Some(candidates) = &api_resp.candidates {
            if let Some(candidate) = candidates.first() {
                finish_reason = candidate.finish_reason.as_ref().map(|r| {
                    match r.as_str() {
                        "STOP" => "stop".into(),
                        "MAX_TOKENS" => "length".into(),
                        other => other.to_lowercase(),
                    }
                });

                if let Some(content) = &candidate.content {
                    for part in &content.parts {
                        match part {
                            GeminiPart::Text(text) => text_parts.push(text.clone()),
                            GeminiPart::FunctionCall { name, args } => {
                                tool_calls.push(ToolCall {
                                    id: format!("call_{}", uuid::Uuid::new_v4()),
                                    r#type: "function".into(),
                                    function: FunctionCall {
                                        name: name.clone(),
                                        arguments: serde_json::to_string(args)
                                            .unwrap_or_default(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let content = text_parts.join("");
        let usage_meta = api_resp.usage_metadata.unwrap_or(GeminiUsageMetadata {
            prompt_token_count: Some(0),
            candidates_token_count: Some(0),
            total_token_count: Some(0),
        });

        let prompt_tokens = usage_meta.prompt_token_count.unwrap_or(0);
        let completion_tokens = usage_meta.candidates_token_count.unwrap_or(0);

        ChatResponse {
            id: format!("gemini-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".into(),
            created: chrono::Utc::now().timestamp(),
            model: model.to_string(),
            choices: vec![Choice {
                index: 0,
                message: ResponseMessage {
                    role: "assistant".into(),
                    content: if content.is_empty() {
                        None
                    } else {
                        Some(content)
                    },
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
                total_tokens: usage_meta.total_token_count.unwrap_or(
                    prompt_tokens + completion_tokens,
                ),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// LlmProvider implementation
// ---------------------------------------------------------------------------

impl<T: TokenStorage + 'static> LlmProvider for GeminiProvider<T> {
    fn id(&self) -> &str {
        "gemini"
    }

    fn name(&self) -> &str {
        "Gemini (Google)"
    }

    fn models(&self) -> Vec<String> {
        SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect()
    }

    fn supports_model(&self, model: &str) -> bool {
        SUPPORTED_MODELS.iter().any(|m| *m == model)
    }

    fn chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        let request = request.clone();
        Box::pin(async move {
            let token = self.get_token().await?;
            let body = Self::convert_request(&request);

            let url = format!("{}/{}:generateContent", API_BASE, request.model);

            let resp = self
                .http
                .post(&url)
                .bearer_auth(&token)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await?;

            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                if status.as_u16() == 429 {
                    return Err(ProviderError::RateLimited {
                        retry_after_secs: 60,
                    });
                }
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message: text,
                });
            }

            let api_resp: GeminiResponse = resp.json().await?;
            Ok(Self::convert_response(&request.model, api_resp))
        })
    }

    fn stream_chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>, ProviderError>> + Send + '_>> {
        let request = request.clone();
        Box::pin(async move {
        let token = self.get_token().await?;
        let body = Self::convert_request(&request);

        let url = format!(
            "{}/{}:streamGenerateContent?alt=sse",
            API_BASE, request.model
        );

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimited {
                    retry_after_secs: 60,
                });
            }
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        let byte_stream = resp.bytes_stream();
        let model = request.model.clone();
        let chunk_id = format!("gemini-{}", uuid::Uuid::new_v4());

        let event_stream = stream::unfold(
            (
                Box::pin(byte_stream.map(move |r| r.map_err(|e| ProviderError::Stream(e.to_string())))),
                String::new(),
                model,
                chunk_id,
            ),
            |(mut inner, mut buf, model, chunk_id)| async move {
                loop {
                    // Parse SSE data lines from buffer.
                    while let Some(newline_pos) = buf.find('\n') {
                        let line = buf[..newline_pos].trim_end_matches('\r').to_string();
                        buf = buf[newline_pos + 1..].to_string();

                        if line.is_empty() {
                            continue;
                        }

                        if let Some(data) = line.strip_prefix("data: ") {
                            if data.trim() == "[DONE]" {
                                return None;
                            }

                            let gemini_resp: GeminiResponse = match serde_json::from_str(data) {
                                Ok(r) => r,
                                Err(e) => {
                                    debug!(error = %e, "Failed to parse Gemini SSE data");
                                    continue;
                                }
                            };

                            // Extract text from first candidate.
                            let mut text = String::new();
                            let mut finish_reason: Option<String> = None;

                            if let Some(candidates) = &gemini_resp.candidates {
                                if let Some(candidate) = candidates.first() {
                                    finish_reason = candidate.finish_reason.as_ref().map(|r| {
                                        match r.as_str() {
                                            "STOP" => "stop".into(),
                                            "MAX_TOKENS" => "length".into(),
                                            other => other.to_lowercase(),
                                        }
                                    });
                                    if let Some(content) = &candidate.content {
                                        for part in &content.parts {
                                            if let GeminiPart::Text(t) = part {
                                                text.push_str(t);
                                            }
                                        }
                                    }
                                }
                            }

                            let usage = gemini_resp.usage_metadata.map(|u| {
                                let pt = u.prompt_token_count.unwrap_or(0);
                                let ct = u.candidates_token_count.unwrap_or(0);
                                Usage {
                                    prompt_tokens: pt,
                                    completion_tokens: ct,
                                    total_tokens: u.total_token_count.unwrap_or(pt + ct),
                                }
                            });

                            if !text.is_empty() || finish_reason.is_some() {
                                let chunk = ChatChunk {
                                    id: chunk_id.clone(),
                                    object: "chat.completion.chunk".into(),
                                    created: chrono::Utc::now().timestamp(),
                                    model: model.clone(),
                                    choices: vec![ChunkChoice {
                                        index: 0,
                                        delta: Delta {
                                            role: None,
                                            content: if text.is_empty() {
                                                None
                                            } else {
                                                Some(text)
                                            },
                                            tool_calls: None,
                                        },
                                        finish_reason,
                                    }],
                                    usage,
                                };
                                return Some((Ok(chunk), (inner, buf, model, chunk_id)));
                            }
                        }
                    }

                    // Need more bytes.
                    match inner.next().await {
                        Some(Ok(bytes)) => {
                            buf.push_str(&String::from_utf8_lossy(&bytes));
                        }
                        Some(Err(e)) => {
                            return Some((Err(e), (inner, buf, model, chunk_id)));
                        }
                        None => return None,
                    }
                }
            },
        );

        Ok(Box::pin(event_stream) as Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>)
        })
    }

    fn health_check(&self) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(async move { self.get_token().await.is_ok() })
    }

    fn pricing(&self) -> Vec<ModelPricing> {
        crate::providers::cost::CostDatabase::all()
            .into_iter()
            .filter(|p| p.provider == "gemini")
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct MockTokenStorage {
        token: Option<String>,
    }

    impl MockTokenStorage {
        fn with_token(token: &str) -> Self {
            Self {
                token: Some(token.into()),
            }
        }

        fn empty() -> Self {
            Self { token: None }
        }
    }

    impl TokenStorage for MockTokenStorage {
        async fn get_access_token(
            &self,
            _provider: &str,
        ) -> Result<Option<String>, ProviderError> {
            Ok(self.token.clone())
        }
    }

    #[test]
    fn test_id_and_name() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        assert_eq!(p.id(), "gemini");
        assert_eq!(p.name(), "Gemini (Google)");
    }

    #[test]
    fn test_models_list() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        let models = p.models();
        assert!(models.contains(&"gemini-2.5-flash".to_string()));
        assert!(models.contains(&"gemini-2.5-pro".to_string()));
        assert!(models.contains(&"gemini-2.0-flash".to_string()));
    }

    #[test]
    fn test_supports_model() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        assert!(p.supports_model("gemini-2.5-flash"));
        assert!(!p.supports_model("gpt-4o"));
    }

    #[tokio::test]
    async fn test_health_check_no_token() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        assert!(!p.health_check().await);
    }

    #[tokio::test]
    async fn test_health_check_with_token() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::with_token("test")));
        assert!(p.health_check().await);
    }

    #[tokio::test]
    async fn test_chat_no_token() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        let req = ChatRequest {
            model: "gemini-2.5-flash".into(),
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
        };
        let result = p.chat(&req).await;
        assert!(matches!(result, Err(ProviderError::NoToken(_))));
    }

    #[test]
    fn test_convert_request_system_instruction() {
        let req = ChatRequest {
            model: "gemini-2.5-flash".into(),
            messages: vec![
                ChatMessage {
                    role: MessageRole::System,
                    content: Some(MessageContent::Text("Be helpful.".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: MessageRole::User,
                    content: Some(MessageContent::Text("Hi".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            temperature: Some(0.5),
            max_tokens: Some(2048),
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
        };

        let converted = GeminiProvider::<MockTokenStorage>::convert_request(&req);
        assert!(converted.system_instruction.is_some());
        assert_eq!(converted.contents.len(), 1); // system is separate
        assert_eq!(
            converted.contents[0].role.as_deref(),
            Some("user")
        );
        let config = converted.generation_config.unwrap();
        assert_eq!(config.temperature, Some(0.5));
        assert_eq!(config.max_output_tokens, Some(2048));
    }

    #[test]
    fn test_convert_response_text() {
        let api_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart::Text("Hello!".into())],
                }),
                finish_reason: Some("STOP".into()),
            }]),
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: Some(10),
                candidates_token_count: Some(5),
                total_token_count: Some(15),
            }),
            model_version: None,
        };

        let resp = GeminiProvider::<MockTokenStorage>::convert_response("gemini-2.5-flash", api_resp);
        assert_eq!(resp.model, "gemini-2.5-flash");
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(
            resp.choices[0].message.content,
            Some("Hello!".to_string())
        );
        assert_eq!(
            resp.choices[0].finish_reason,
            Some("stop".to_string())
        );
        assert_eq!(resp.usage.prompt_tokens, 10);
        assert_eq!(resp.usage.completion_tokens, 5);
    }

    #[test]
    fn test_convert_response_function_call() {
        let api_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart::FunctionCall {
                        name: "search".into(),
                        args: serde_json::json!({"query": "weather"}),
                    }],
                }),
                finish_reason: Some("STOP".into()),
            }]),
            usage_metadata: None,
            model_version: None,
        };

        let resp = GeminiProvider::<MockTokenStorage>::convert_response("gemini-2.5-flash", api_resp);
        let tc = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].function.name, "search");
    }

    #[test]
    fn test_pricing_returns_gemini_models() {
        let p = GeminiProvider::new(Arc::new(MockTokenStorage::empty()));
        let pricing = p.pricing();
        assert!(!pricing.is_empty());
        for mp in &pricing {
            assert_eq!(mp.provider, "gemini");
        }
    }

    #[test]
    fn test_convert_request_with_tools() {
        let req = ChatRequest {
            model: "gemini-2.5-flash".into(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: None,
            tools: Some(vec![Tool {
                r#type: "function".into(),
                function: FunctionDef {
                    name: "get_time".into(),
                    description: Some("Get current time".into()),
                    parameters: Some(serde_json::json!({"type": "object", "properties": {}})),
                },
            }]),
            tool_choice: None,
        };

        let converted = GeminiProvider::<MockTokenStorage>::convert_request(&req);
        assert!(converted.tools.is_some());
        let tools = converted.tools.unwrap();
        assert_eq!(tools[0].function_declarations.len(), 1);
        assert_eq!(tools[0].function_declarations[0].name, "get_time");
    }

    #[test]
    fn test_convert_response_no_candidates() {
        let api_resp = GeminiResponse {
            candidates: None,
            usage_metadata: None,
            model_version: None,
        };
        let resp = GeminiProvider::<MockTokenStorage>::convert_response("gemini-2.0-flash", api_resp);
        assert!(resp.choices[0].message.content.is_none());
        assert!(resp.choices[0].finish_reason.is_none());
    }
}
