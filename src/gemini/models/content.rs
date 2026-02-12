//! Content block types for messages.
//!
//! This module provides the `ContentBlock` enum and related types that represent
//! the different kinds of content that can appear in messages.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A content block within a message.
///
/// Messages can contain multiple content blocks of different types.
/// This enum represents all supported content block variants in the
/// Anthropic Messages API format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text content.
    Text {
        /// The text content.
        text: String,
    },

    /// A tool use request from the assistant.
    ToolUse {
        /// Unique identifier for this tool use.
        id: String,
        /// Name of the tool being called.
        name: String,
        /// Input arguments for the tool (as JSON).
        input: Value,
    },

    /// Result from a tool call (user provides this in response to tool_use).
    ToolResult {
        /// The ID of the tool_use block this is responding to.
        tool_use_id: String,
        /// The result content from the tool.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<ToolResultContent>,
        /// Whether the tool call resulted in an error.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },

    /// Thinking/reasoning content from extended thinking models.
    Thinking {
        /// The model's internal reasoning process.
        thinking: String,
        /// Signature for thinking block continuity across turns.
        /// Required for Claude thinking models when sending back thinking blocks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// Image content (base64 encoded or URL).
    Image {
        /// The image source.
        source: ImageSource,
    },

    /// Document content (base64 encoded).
    Document {
        /// The document source.
        source: DocumentSource,
    },
}

impl ContentBlock {
    /// Create a text content block.
    pub fn text(text: impl Into<String>) -> Self {
        ContentBlock::Text { text: text.into() }
    }

    /// Create a tool use content block.
    pub fn tool_use(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    /// Create a tool result content block with text content.
    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        ContentBlock::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: Some(ToolResultContent::Text(content.into())),
            is_error: None,
        }
    }

    /// Create a tool result content block indicating an error.
    pub fn tool_result_error(tool_use_id: impl Into<String>, error: impl Into<String>) -> Self {
        ContentBlock::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: Some(ToolResultContent::Text(error.into())),
            is_error: Some(true),
        }
    }

    /// Create a thinking content block.
    pub fn thinking(thinking: impl Into<String>, signature: Option<String>) -> Self {
        ContentBlock::Thinking {
            thinking: thinking.into(),
            signature,
        }
    }

    /// Create an image content block from base64 data.
    pub fn image_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: media_type.into(),
                data: data.into(),
            },
        }
    }

    /// Create an image content block from a URL.
    pub fn image_url(url: impl Into<String>) -> Self {
        ContentBlock::Image {
            source: ImageSource::Url { url: url.into() },
        }
    }

    /// Create a document content block from base64 data.
    pub fn document_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        ContentBlock::Document {
            source: DocumentSource::Base64 {
                media_type: media_type.into(),
                data: data.into(),
            },
        }
    }

    /// Check if this is a text block.
    pub fn is_text(&self) -> bool {
        matches!(self, ContentBlock::Text { .. })
    }

    /// Check if this is a tool use block.
    pub fn is_tool_use(&self) -> bool {
        matches!(self, ContentBlock::ToolUse { .. })
    }

    /// Check if this is a tool result block.
    pub fn is_tool_result(&self) -> bool {
        matches!(self, ContentBlock::ToolResult { .. })
    }

    /// Check if this is a thinking block.
    pub fn is_thinking(&self) -> bool {
        matches!(self, ContentBlock::Thinking { .. })
    }

    /// Check if this is an image block.
    pub fn is_image(&self) -> bool {
        matches!(self, ContentBlock::Image { .. })
    }

    /// Check if this is a document block.
    pub fn is_document(&self) -> bool {
        matches!(self, ContentBlock::Document { .. })
    }

    /// Get the text content if this is a text block.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Get the thinking content if this is a thinking block.
    pub fn as_thinking(&self) -> Option<&str> {
        match self {
            ContentBlock::Thinking { thinking, .. } => Some(thinking),
            _ => None,
        }
    }

    /// Get the tool use details if this is a tool use block.
    pub fn as_tool_use(&self) -> Option<(&str, &str, &Value)> {
        match self {
            ContentBlock::ToolUse { id, name, input } => Some((id, name, input)),
            _ => None,
        }
    }

    /// Get the tool result details if this is a tool result block.
    pub fn as_tool_result(&self) -> Option<(&str, Option<&ToolResultContent>, bool)> {
        match self {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((tool_use_id, content.as_ref(), is_error.unwrap_or(false))),
            _ => None,
        }
    }
}

/// Source for image content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Base64-encoded image data.
    Base64 {
        /// MIME type of the image (e.g., "image/png", "image/jpeg").
        media_type: String,
        /// Base64-encoded image data.
        data: String,
    },

    /// URL reference to an image.
    Url {
        /// URL of the image.
        url: String,
    },
}

impl ImageSource {
    /// Check if this is a base64 source.
    pub fn is_base64(&self) -> bool {
        matches!(self, ImageSource::Base64 { .. })
    }

    /// Check if this is a URL source.
    pub fn is_url(&self) -> bool {
        matches!(self, ImageSource::Url { .. })
    }
}

/// Source for document content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocumentSource {
    /// Base64-encoded document data.
    Base64 {
        /// MIME type of the document (e.g., "application/pdf").
        media_type: String,
        /// Base64-encoded document data.
        data: String,
    },
}

impl DocumentSource {
    /// Get the media type of the document.
    pub fn media_type(&self) -> &str {
        match self {
            DocumentSource::Base64 { media_type, .. } => media_type,
        }
    }

    /// Get the base64 data of the document.
    pub fn data(&self) -> &str {
        match self {
            DocumentSource::Base64 { data, .. } => data,
        }
    }
}

/// Content for a tool result.
///
/// Tool results can be simple text or a collection of content blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ToolResultContent {
    /// Simple text result.
    Text(String),
    /// Multiple content blocks (can include text, images, etc.).
    Blocks(Vec<ContentBlock>),
}

impl ToolResultContent {
    /// Create a text tool result.
    pub fn text(text: impl Into<String>) -> Self {
        ToolResultContent::Text(text.into())
    }

    /// Create a blocks tool result.
    pub fn blocks(blocks: Vec<ContentBlock>) -> Self {
        ToolResultContent::Blocks(blocks)
    }

    /// Check if this is a text result.
    pub fn is_text(&self) -> bool {
        matches!(self, ToolResultContent::Text(_))
    }

    /// Check if this is a blocks result.
    pub fn is_blocks(&self) -> bool {
        matches!(self, ToolResultContent::Blocks(_))
    }

    /// Get the text content if this is a text result.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ToolResultContent::Text(text) => Some(text),
            _ => None,
        }
    }

    /// Get the blocks if this is a blocks result.
    pub fn as_blocks(&self) -> Option<&[ContentBlock]> {
        match self {
            ToolResultContent::Blocks(blocks) => Some(blocks),
            _ => None,
        }
    }

    /// Convert to a text representation.
    ///
    /// For text results, returns the text directly.
    /// For block results, concatenates all text blocks.
    pub fn to_text(&self) -> String {
        match self {
            ToolResultContent::Text(text) => text.clone(),
            ToolResultContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| b.as_text())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

impl From<String> for ToolResultContent {
    fn from(text: String) -> Self {
        ToolResultContent::Text(text)
    }
}

impl From<&str> for ToolResultContent {
    fn from(text: &str) -> Self {
        ToolResultContent::Text(text.to_string())
    }
}

impl From<Vec<ContentBlock>> for ToolResultContent {
    fn from(blocks: Vec<ContentBlock>) -> Self {
        ToolResultContent::Blocks(blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_text_block() {
        let block = ContentBlock::text("Hello, world!");
        assert!(block.is_text());
        assert!(!block.is_tool_use());
        assert_eq!(block.as_text(), Some("Hello, world!"));

        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "Hello, world!");
    }

    #[test]
    fn test_tool_use_block() {
        let block = ContentBlock::tool_use(
            "toolu_123",
            "get_weather",
            json!({"location": "San Francisco"}),
        );

        assert!(block.is_tool_use());
        let (id, name, input) = block.as_tool_use().unwrap();
        assert_eq!(id, "toolu_123");
        assert_eq!(name, "get_weather");
        assert_eq!(input["location"], "San Francisco");

        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_use");
        assert_eq!(json["id"], "toolu_123");
        assert_eq!(json["name"], "get_weather");
    }

    #[test]
    fn test_tool_result_block() {
        let block = ContentBlock::tool_result("toolu_123", "The weather is sunny");

        assert!(block.is_tool_result());
        let (id, content, is_error) = block.as_tool_result().unwrap();
        assert_eq!(id, "toolu_123");
        assert_eq!(content.unwrap().as_text(), Some("The weather is sunny"));
        assert!(!is_error);

        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["tool_use_id"], "toolu_123");
    }

    #[test]
    fn test_tool_result_error_block() {
        let block = ContentBlock::tool_result_error("toolu_123", "API call failed");

        let (_, _, is_error) = block.as_tool_result().unwrap();
        assert!(is_error);

        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["is_error"], true);
    }

    #[test]
    fn test_thinking_block() {
        let block = ContentBlock::thinking("Let me analyze this...", Some("sig123".to_string()));

        assert!(block.is_thinking());
        assert_eq!(block.as_thinking(), Some("Let me analyze this..."));

        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "thinking");
        assert_eq!(json["thinking"], "Let me analyze this...");
        assert_eq!(json["signature"], "sig123");
    }

    #[test]
    fn test_thinking_block_without_signature() {
        let block = ContentBlock::thinking("Thinking...", None);

        let json = serde_json::to_string(&block).unwrap();
        // signature should be omitted when None
        assert!(!json.contains("signature"));
    }

    #[test]
    fn test_image_block_base64() {
        let block = ContentBlock::image_base64("image/png", "iVBORw0KGgo=");

        assert!(block.is_image());
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "image");
        assert_eq!(json["source"]["type"], "base64");
        assert_eq!(json["source"]["media_type"], "image/png");
        assert_eq!(json["source"]["data"], "iVBORw0KGgo=");
    }

    #[test]
    fn test_image_block_url() {
        let block = ContentBlock::image_url("https://example.com/image.png");

        assert!(block.is_image());
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "image");
        assert_eq!(json["source"]["type"], "url");
        assert_eq!(json["source"]["url"], "https://example.com/image.png");
    }

    #[test]
    fn test_document_block() {
        let block = ContentBlock::document_base64("application/pdf", "JVBERi0=");

        assert!(block.is_document());
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "document");
        assert_eq!(json["source"]["type"], "base64");
        assert_eq!(json["source"]["media_type"], "application/pdf");
    }

    #[test]
    fn test_content_block_deserialization() {
        let text_json = r#"{"type": "text", "text": "Hello"}"#;
        let text: ContentBlock = serde_json::from_str(text_json).unwrap();
        assert!(text.is_text());

        let tool_use_json =
            r#"{"type": "tool_use", "id": "t1", "name": "calc", "input": {"x": 1}}"#;
        let tool_use: ContentBlock = serde_json::from_str(tool_use_json).unwrap();
        assert!(tool_use.is_tool_use());

        let thinking_json = r#"{"type": "thinking", "thinking": "Hmm..."}"#;
        let thinking: ContentBlock = serde_json::from_str(thinking_json).unwrap();
        assert!(thinking.is_thinking());
    }

    #[test]
    fn test_content_block_roundtrip() {
        let blocks = vec![
            ContentBlock::text("test"),
            ContentBlock::tool_use("id", "name", json!({})),
            ContentBlock::tool_result("id", "result"),
            ContentBlock::thinking("thought", Some("sig".to_string())),
            ContentBlock::image_base64("image/png", "data"),
            ContentBlock::image_url("https://example.com/img.png"),
            ContentBlock::document_base64("application/pdf", "data"),
        ];

        for original in blocks {
            let serialized = serde_json::to_string(&original).unwrap();
            let deserialized: ContentBlock = serde_json::from_str(&serialized).unwrap();
            assert_eq!(original, deserialized);
        }
    }

    #[test]
    fn test_image_source_helpers() {
        let base64 = ImageSource::Base64 {
            media_type: "image/png".to_string(),
            data: "abc".to_string(),
        };
        assert!(base64.is_base64());
        assert!(!base64.is_url());

        let url = ImageSource::Url {
            url: "https://example.com".to_string(),
        };
        assert!(url.is_url());
        assert!(!url.is_base64());
    }

    #[test]
    fn test_document_source_helpers() {
        let source = DocumentSource::Base64 {
            media_type: "application/pdf".to_string(),
            data: "JVBERi0=".to_string(),
        };

        assert_eq!(source.media_type(), "application/pdf");
        assert_eq!(source.data(), "JVBERi0=");
    }

    #[test]
    fn test_tool_result_content_text() {
        let content = ToolResultContent::text("Result text");
        assert!(content.is_text());
        assert!(!content.is_blocks());
        assert_eq!(content.as_text(), Some("Result text"));
        assert_eq!(content.to_text(), "Result text");

        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, "Result text");
    }

    #[test]
    fn test_tool_result_content_blocks() {
        let content = ToolResultContent::blocks(vec![
            ContentBlock::text("Line 1"),
            ContentBlock::text("Line 2"),
        ]);

        assert!(content.is_blocks());
        assert!(!content.is_text());
        assert_eq!(content.as_blocks().unwrap().len(), 2);
        assert_eq!(content.to_text(), "Line 1\nLine 2");
    }

    #[test]
    fn test_tool_result_content_from_impls() {
        let from_string: ToolResultContent = "text".to_string().into();
        assert!(from_string.is_text());

        let from_str: ToolResultContent = "text".into();
        assert!(from_str.is_text());

        let from_vec: ToolResultContent = vec![ContentBlock::text("block")].into();
        assert!(from_vec.is_blocks());
    }

    #[test]
    fn test_tool_result_content_deserialization() {
        // Text form
        let text: ToolResultContent = serde_json::from_str(r#""simple text""#).unwrap();
        assert!(text.is_text());
        assert_eq!(text.as_text(), Some("simple text"));

        // Blocks form
        let blocks: ToolResultContent =
            serde_json::from_str(r#"[{"type": "text", "text": "block text"}]"#).unwrap();
        assert!(blocks.is_blocks());
    }

    #[test]
    fn test_tool_result_empty_content() {
        // Tool result can have no content field
        let json = r#"{"type": "tool_result", "tool_use_id": "t1"}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();

        if let ContentBlock::ToolResult { content, .. } = block {
            assert!(content.is_none());
        } else {
            panic!("Expected ToolResult");
        }
    }
}
