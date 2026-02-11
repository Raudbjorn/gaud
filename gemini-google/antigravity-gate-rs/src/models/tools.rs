//! Tool definitions for function calling.
//!
//! This module provides types for defining tools that can be called by the model.
//! Tools follow the Anthropic Messages API format.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool that can be called by the model.
///
/// Tools enable the model to interact with external systems by requesting
/// function calls with structured arguments.
///
/// # Example
///
/// ```rust
/// use antigravity_gate::models::Tool;
/// use serde_json::json;
///
/// let tool = Tool::new(
///     "get_weather",
///     "Get the current weather for a location",
///     json!({
///         "type": "object",
///         "properties": {
///             "location": {
///                 "type": "string",
///                 "description": "The city and state, e.g. San Francisco, CA"
///             }
///         },
///         "required": ["location"]
///     }),
/// );
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tool {
    /// The name of the tool. Must be unique within a request.
    pub name: String,

    /// A description of what the tool does.
    /// The model uses this to decide when and how to use the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// JSON Schema describing the expected input parameters.
    pub input_schema: Value,
}

impl Tool {
    /// Create a new tool with the given name, description, and input schema.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
            input_schema,
        }
    }

    /// Create a new tool with only a name and input schema (no description).
    pub fn with_name_and_schema(name: impl Into<String>, input_schema: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            input_schema,
        }
    }
}

/// Specifies how the model should use tools.
///
/// This controls whether the model is required to use a tool, free to choose,
/// or directed to use a specific tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    /// Let the model decide whether to use a tool.
    #[default]
    Auto,

    /// Force the model to use one of the provided tools.
    Any,

    /// Force the model to use a specific tool by name.
    Tool {
        /// The name of the tool to use.
        name: String,
    },

    /// Disable tool use entirely.
    None,
}

impl ToolChoice {
    /// Create a `ToolChoice::Tool` variant for a specific tool name.
    pub fn tool(name: impl Into<String>) -> Self {
        ToolChoice::Tool { name: name.into() }
    }

    /// Check if this is the `Auto` variant.
    pub fn is_auto(&self) -> bool {
        matches!(self, ToolChoice::Auto)
    }

    /// Check if this is the `Any` variant.
    pub fn is_any(&self) -> bool {
        matches!(self, ToolChoice::Any)
    }

    /// Check if this forces a specific tool.
    pub fn is_specific_tool(&self) -> bool {
        matches!(self, ToolChoice::Tool { .. })
    }

    /// Check if this is the `None` variant (tools disabled).
    pub fn is_none(&self) -> bool {
        matches!(self, ToolChoice::None)
    }

    /// Get the tool name if this is a specific tool choice.
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            ToolChoice::Tool { name } => Some(name),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_creation() {
        let schema = json!({
            "type": "object",
            "properties": {
                "location": {"type": "string"}
            },
            "required": ["location"]
        });

        let tool = Tool::new("get_weather", "Get current weather", schema.clone());
        assert_eq!(tool.name, "get_weather");
        assert_eq!(tool.description, Some("Get current weather".to_string()));
        assert_eq!(tool.input_schema, schema);
    }

    #[test]
    fn test_tool_without_description() {
        let schema = json!({"type": "object"});
        let tool = Tool::with_name_and_schema("simple_tool", schema.clone());

        assert_eq!(tool.name, "simple_tool");
        assert_eq!(tool.description, None);
        assert_eq!(tool.input_schema, schema);
    }

    #[test]
    fn test_tool_serialization() {
        let tool = Tool::new(
            "test_tool",
            "A test tool",
            json!({"type": "object", "properties": {}}),
        );

        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["name"], "test_tool");
        assert_eq!(json["description"], "A test tool");
        assert!(json["input_schema"].is_object());
    }

    #[test]
    fn test_tool_deserialization() {
        let json = r#"{
            "name": "calculator",
            "description": "Perform calculations",
            "input_schema": {
                "type": "object",
                "properties": {
                    "expression": {"type": "string"}
                }
            }
        }"#;

        let tool: Tool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "calculator");
        assert_eq!(tool.description, Some("Perform calculations".to_string()));
    }

    #[test]
    fn test_tool_roundtrip() {
        let original = Tool::new(
            "search",
            "Search the web",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer"}
                },
                "required": ["query"]
            }),
        );

        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: Tool = serde_json::from_str(&serialized).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_tool_without_description_serialization() {
        let tool = Tool::with_name_and_schema("minimal", json!({"type": "object"}));
        let json = serde_json::to_string(&tool).unwrap();

        // Description should be omitted from JSON
        assert!(!json.contains("description"));
    }

    #[test]
    fn test_tool_choice_auto() {
        let choice = ToolChoice::Auto;
        assert!(choice.is_auto());
        assert!(!choice.is_any());
        assert!(!choice.is_specific_tool());
        assert_eq!(choice.tool_name(), None);

        let json = serde_json::to_value(&choice).unwrap();
        assert_eq!(json["type"], "auto");
    }

    #[test]
    fn test_tool_choice_any() {
        let choice = ToolChoice::Any;
        assert!(choice.is_any());
        assert!(!choice.is_auto());
        assert!(!choice.is_specific_tool());

        let json = serde_json::to_value(&choice).unwrap();
        assert_eq!(json["type"], "any");
    }

    #[test]
    fn test_tool_choice_specific() {
        let choice = ToolChoice::tool("get_weather");
        assert!(choice.is_specific_tool());
        assert!(!choice.is_auto());
        assert!(!choice.is_any());
        assert_eq!(choice.tool_name(), Some("get_weather"));

        let json = serde_json::to_value(&choice).unwrap();
        assert_eq!(json["type"], "tool");
        assert_eq!(json["name"], "get_weather");
    }

    #[test]
    fn test_tool_choice_none() {
        let choice = ToolChoice::None;
        assert!(choice.is_none());
        assert!(!choice.is_auto());
        assert!(!choice.is_any());

        let json = serde_json::to_value(&choice).unwrap();
        assert_eq!(json["type"], "none");
    }

    #[test]
    fn test_tool_choice_default() {
        let choice = ToolChoice::default();
        assert!(choice.is_auto());
    }

    #[test]
    fn test_tool_choice_deserialization() {
        let auto: ToolChoice = serde_json::from_str(r#"{"type": "auto"}"#).unwrap();
        assert!(auto.is_auto());

        let any: ToolChoice = serde_json::from_str(r#"{"type": "any"}"#).unwrap();
        assert!(any.is_any());

        let tool: ToolChoice =
            serde_json::from_str(r#"{"type": "tool", "name": "calculator"}"#).unwrap();
        assert_eq!(tool.tool_name(), Some("calculator"));

        let none: ToolChoice = serde_json::from_str(r#"{"type": "none"}"#).unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn test_tool_choice_roundtrip() {
        let choices = vec![
            ToolChoice::Auto,
            ToolChoice::Any,
            ToolChoice::tool("my_tool"),
            ToolChoice::None,
        ];

        for original in choices {
            let serialized = serde_json::to_string(&original).unwrap();
            let deserialized: ToolChoice = serde_json::from_str(&serialized).unwrap();
            assert_eq!(original, deserialized);
        }
    }
}
