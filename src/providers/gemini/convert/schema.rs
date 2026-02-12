//! JSON Schema sanitization for Google API compatibility.
//!
//! This module provides functions to clean JSON Schema definitions for use
//! with Google's Generative AI API, which has stricter schema requirements
//! than the full JSON Schema specification.
//!
//! ## Supported Features
//!
//! The following JSON Schema keywords are preserved:
//! - `type` (converted to uppercase: STRING, OBJECT, ARRAY, etc.)
//! - `description`
//! - `properties`
//! - `required`
//! - `items`
//! - `enum`
//!
//! ## Unsupported Features
//!
//! The following are removed or converted:
//! - `$ref`, `$defs`, `$schema`, `$id` - Reference-based schemas
//! - `additionalProperties` - Extra property control
//! - `allOf`, `anyOf`, `oneOf`, `not` - Schema composition
//! - `default`, `examples`, `const` - Value constraints (const -> enum)
//! - `minLength`, `maxLength`, `pattern`, `format` - String constraints
//! - `minimum`, `maximum`, `minItems`, `maxItems` - Numeric constraints
//! - `if`, `then`, `else` - Conditional schemas
//!
//! ## Example
//!
//! ```rust
//! use serde_json::json;
//! use gaud::providers::gemini::convert::sanitize_schema;
//!
//! let schema = json!({
//!     "type": "object",
//!     "additionalProperties": false,
//!     "properties": {
//!         "name": {
//!             "type": "string",
//!             "minLength": 1,
//!             "maxLength": 100
//!         }
//!     },
//!     "required": ["name"]
//! });
//!
//! let sanitized = sanitize_schema(&schema);
//! // Result: { "type": "OBJECT", "properties": { "name": { "type": "STRING" } }, "required": ["name"] }
//! ```

use serde_json::{Map, Value};

/// Keywords that are allowed in Google API schemas.
const ALLOWED_KEYWORDS: &[&str] = &[
    "type",
    "description",
    "properties",
    "required",
    "items",
    "enum",
];

/// Keywords that should be removed from schemas.
const UNSUPPORTED_KEYWORDS: &[&str] = &[
    "additionalProperties",
    "default",
    "$schema",
    "$defs",
    "definitions",
    "$ref",
    "$id",
    "$comment",
    "title",
    "minLength",
    "maxLength",
    "pattern",
    "format",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minItems",
    "maxItems",
    "uniqueItems",
    "examples",
    "allOf",
    "anyOf",
    "oneOf",
    "not",
    "if",
    "then",
    "else",
    "dependentSchemas",
    "dependentRequired",
    "propertyNames",
    "unevaluatedItems",
    "unevaluatedProperties",
    "contentMediaType",
    "contentEncoding",
    "deprecated",
    "readOnly",
    "writeOnly",
];

/// Sanitize a JSON Schema for Google API compatibility.
///
/// This function removes unsupported keywords and converts types to
/// Google's uppercase format (STRING, OBJECT, ARRAY, etc.).
///
/// # Arguments
///
/// * `schema` - The JSON Schema to sanitize
///
/// # Returns
///
/// A sanitized schema suitable for Google's Generative AI API.
/// If the input is null or not an object, returns a placeholder schema.
pub fn sanitize_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(obj) => sanitize_object(obj),
        Value::Null => create_placeholder_schema(),
        _ => create_placeholder_schema(),
    }
}

/// Sanitize a schema object recursively.
fn sanitize_object(obj: &Map<String, Value>) -> Value {
    // Handle empty schema
    if obj.is_empty() {
        return create_placeholder_schema();
    }

    let mut result = Map::new();

    for (key, value) in obj {
        // Handle const -> enum conversion
        if key == "const" {
            result.insert("enum".to_string(), Value::Array(vec![value.clone()]));
            continue;
        }

        // Skip unsupported keywords
        if UNSUPPORTED_KEYWORDS.contains(&key.as_str()) {
            continue;
        }

        // Only keep allowed keywords (or process special cases)
        if !ALLOWED_KEYWORDS.contains(&key.as_str()) {
            continue;
        }

        match key.as_str() {
            "type" => {
                let google_type = convert_type_to_google(value);
                result.insert("type".to_string(), Value::String(google_type));
            }
            "properties" => {
                if let Value::Object(props) = value {
                    let sanitized_props = sanitize_properties(props);
                    result.insert("properties".to_string(), Value::Object(sanitized_props));
                }
            }
            "items" => {
                let sanitized_items = sanitize_items(value);
                result.insert("items".to_string(), sanitized_items);
            }
            "required" => {
                // Keep required array as-is (will be validated later)
                result.insert("required".to_string(), value.clone());
            }
            "enum" => {
                result.insert("enum".to_string(), value.clone());
            }
            "description" => {
                result.insert("description".to_string(), value.clone());
            }
            _ => {
                // For other allowed keywords, pass through
                result.insert(key.clone(), value.clone());
            }
        }
    }

    // Ensure we have a type
    if !result.contains_key("type") {
        result.insert("type".to_string(), Value::String("OBJECT".to_string()));
    }

    // If object type with no properties, add placeholder
    let is_object = result
        .get("type")
        .and_then(|v| v.as_str())
        .map(|s| s == "OBJECT" || s == "object")
        .unwrap_or(false);

    if is_object {
        let has_properties = result
            .get("properties")
            .and_then(|v| v.as_object())
            .map(|p| !p.is_empty())
            .unwrap_or(false);

        if !has_properties {
            result.insert("properties".to_string(), create_reason_property());
            result.insert(
                "required".to_string(),
                Value::Array(vec![Value::String("reason".to_string())]),
            );
        }
    }

    // Validate that required array only contains existing properties
    if let (Some(Value::Array(required)), Some(Value::Object(props))) =
        (result.get("required"), result.get("properties"))
    {
        let valid_required: Vec<Value> = required
            .iter()
            .filter(|r| {
                r.as_str()
                    .map(|name| props.contains_key(name))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        if valid_required.is_empty() {
            result.remove("required");
        } else if valid_required.len() != required.len() {
            result.insert("required".to_string(), Value::Array(valid_required));
        }
    }

    Value::Object(result)
}

/// Sanitize the properties object.
fn sanitize_properties(props: &Map<String, Value>) -> Map<String, Value> {
    let mut result = Map::new();
    for (key, value) in props {
        result.insert(key.clone(), sanitize_schema(value));
    }
    result
}

/// Sanitize the items field (can be object or array).
fn sanitize_items(items: &Value) -> Value {
    match items {
        Value::Object(_) => sanitize_schema(items),
        Value::Array(arr) => {
            let sanitized: Vec<Value> = arr.iter().map(sanitize_schema).collect();
            Value::Array(sanitized)
        }
        _ => sanitize_schema(items),
    }
}

/// Convert a JSON Schema type to Google's uppercase format.
fn convert_type_to_google(type_value: &Value) -> String {
    match type_value {
        Value::String(s) => type_string_to_google(s),
        Value::Array(arr) => {
            // Handle array types like ["string", "null"]
            // Find first non-null type
            for item in arr {
                if let Value::String(s) = item {
                    if s != "null" {
                        return type_string_to_google(s);
                    }
                }
            }
            // Fallback to STRING if only null types
            "STRING".to_string()
        }
        _ => "OBJECT".to_string(),
    }
}

/// Convert a type string to Google format.
fn type_string_to_google(type_str: &str) -> String {
    match type_str.to_lowercase().as_str() {
        "string" => "STRING".to_string(),
        "number" => "NUMBER".to_string(),
        "integer" => "INTEGER".to_string(),
        "boolean" => "BOOLEAN".to_string(),
        "array" => "ARRAY".to_string(),
        "object" => "OBJECT".to_string(),
        "null" => "STRING".to_string(), // Fallback for null type
        _ => type_str.to_uppercase(),
    }
}

/// Create a placeholder schema for empty or invalid schemas.
fn create_placeholder_schema() -> Value {
    let mut props = Map::new();
    props.insert("reason".to_string(), {
        let mut reason = Map::new();
        reason.insert("type".to_string(), Value::String("STRING".to_string()));
        reason.insert(
            "description".to_string(),
            Value::String("Reason for calling this tool".to_string()),
        );
        Value::Object(reason)
    });

    let mut schema = Map::new();
    schema.insert("type".to_string(), Value::String("OBJECT".to_string()));
    schema.insert("properties".to_string(), Value::Object(props));
    schema.insert(
        "required".to_string(),
        Value::Array(vec![Value::String("reason".to_string())]),
    );

    Value::Object(schema)
}

/// Create the reason property for placeholder schemas.
fn create_reason_property() -> Value {
    let mut props = Map::new();
    props.insert("reason".to_string(), {
        let mut reason = Map::new();
        reason.insert("type".to_string(), Value::String("STRING".to_string()));
        reason.insert(
            "description".to_string(),
            Value::String("Reason for calling this tool".to_string()),
        );
        Value::Object(reason)
    });
    Value::Object(props)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_basic_schema_sanitization() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        });

        let result = sanitize_schema(&schema);

        assert_eq!(result["type"], "OBJECT");
        assert_eq!(result["properties"]["name"]["type"], "STRING");
        assert_eq!(result["required"], json!(["name"]));
    }

    #[test]
    fn test_removes_additional_properties() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "name": { "type": "string" }
            }
        });

        let result = sanitize_schema(&schema);

        assert!(result.get("additionalProperties").is_none());
    }

    #[test]
    fn test_removes_refs_and_defs() {
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$defs": {
                "Name": { "type": "string" }
            },
            "$ref": "#/$defs/Name"
        });

        let result = sanitize_schema(&schema);

        assert!(result.get("$schema").is_none());
        assert!(result.get("$defs").is_none());
        assert!(result.get("$ref").is_none());
    }

    #[test]
    fn test_removes_string_constraints() {
        let schema = json!({
            "type": "string",
            "minLength": 1,
            "maxLength": 100,
            "pattern": "^[a-z]+$",
            "format": "email"
        });

        let result = sanitize_schema(&schema);

        assert!(result.get("minLength").is_none());
        assert!(result.get("maxLength").is_none());
        assert!(result.get("pattern").is_none());
        assert!(result.get("format").is_none());
        assert_eq!(result["type"], "STRING");
    }

    #[test]
    fn test_removes_numeric_constraints() {
        let schema = json!({
            "type": "number",
            "minimum": 0,
            "maximum": 100,
            "exclusiveMinimum": 0,
            "exclusiveMaximum": 100
        });

        let result = sanitize_schema(&schema);

        assert!(result.get("minimum").is_none());
        assert!(result.get("maximum").is_none());
        assert!(result.get("exclusiveMinimum").is_none());
        assert!(result.get("exclusiveMaximum").is_none());
        assert_eq!(result["type"], "NUMBER");
    }

    #[test]
    fn test_removes_array_constraints() {
        let schema = json!({
            "type": "array",
            "items": { "type": "string" },
            "minItems": 1,
            "maxItems": 10,
            "uniqueItems": true
        });

        let result = sanitize_schema(&schema);

        assert!(result.get("minItems").is_none());
        assert!(result.get("maxItems").is_none());
        assert!(result.get("uniqueItems").is_none());
        assert_eq!(result["type"], "ARRAY");
        assert_eq!(result["items"]["type"], "STRING");
    }

    #[test]
    fn test_removes_composition_keywords() {
        let schema = json!({
            "allOf": [
                { "type": "object" },
                { "properties": { "name": { "type": "string" } } }
            ]
        });

        let result = sanitize_schema(&schema);

        assert!(result.get("allOf").is_none());
    }

    #[test]
    fn test_converts_const_to_enum() {
        let schema = json!({
            "type": "string",
            "const": "fixed_value"
        });

        let result = sanitize_schema(&schema);

        assert!(result.get("const").is_none());
        assert_eq!(result["enum"], json!(["fixed_value"]));
    }

    #[test]
    fn test_preserves_enum() {
        let schema = json!({
            "type": "string",
            "enum": ["option1", "option2", "option3"]
        });

        let result = sanitize_schema(&schema);

        assert_eq!(result["enum"], json!(["option1", "option2", "option3"]));
    }

    #[test]
    fn test_preserves_description() {
        let schema = json!({
            "type": "string",
            "description": "A user's name"
        });

        let result = sanitize_schema(&schema);

        assert_eq!(result["description"], "A user's name");
    }

    #[test]
    fn test_handles_nested_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "name": {
                            "type": "string",
                            "minLength": 1
                        }
                    }
                }
            }
        });

        let result = sanitize_schema(&schema);

        // Nested additionalProperties and minLength should be removed
        assert!(result["properties"]["user"]
            .get("additionalProperties")
            .is_none());
        assert!(result["properties"]["user"]["properties"]["name"]
            .get("minLength")
            .is_none());
    }

    #[test]
    fn test_handles_array_items() {
        let schema = json!({
            "type": "array",
            "items": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "id": { "type": "integer" }
                }
            }
        });

        let result = sanitize_schema(&schema);

        assert!(result["items"].get("additionalProperties").is_none());
        assert_eq!(result["items"]["properties"]["id"]["type"], "INTEGER");
    }

    #[test]
    fn test_empty_schema_gets_placeholder() {
        let schema = json!({});

        let result = sanitize_schema(&schema);

        assert_eq!(result["type"], "OBJECT");
        assert!(result["properties"]["reason"].is_object());
        assert_eq!(result["required"], json!(["reason"]));
    }

    #[test]
    fn test_null_schema_gets_placeholder() {
        let schema = Value::Null;

        let result = sanitize_schema(&schema);

        assert_eq!(result["type"], "OBJECT");
        assert!(result["properties"]["reason"].is_object());
    }

    #[test]
    fn test_object_without_properties_gets_placeholder() {
        let schema = json!({
            "type": "object"
        });

        let result = sanitize_schema(&schema);

        assert!(result["properties"]["reason"].is_object());
        assert_eq!(result["required"], json!(["reason"]));
    }

    #[test]
    fn test_validates_required_against_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name", "nonexistent"]
        });

        let result = sanitize_schema(&schema);

        // Only "name" should remain in required
        assert_eq!(result["required"], json!(["name"]));
    }

    #[test]
    fn test_removes_required_if_all_invalid() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["nonexistent1", "nonexistent2"]
        });

        let result = sanitize_schema(&schema);

        // required should be removed entirely
        assert!(result.get("required").is_none());
    }

    #[test]
    fn test_type_array_with_null() {
        let schema = json!({
            "type": ["string", "null"]
        });

        let result = sanitize_schema(&schema);

        // Should use first non-null type
        assert_eq!(result["type"], "STRING");
    }

    #[test]
    fn test_all_type_conversions() {
        let types = vec![
            ("string", "STRING"),
            ("number", "NUMBER"),
            ("integer", "INTEGER"),
            ("boolean", "BOOLEAN"),
            ("array", "ARRAY"),
            ("object", "OBJECT"),
        ];

        for (input, expected) in types {
            let schema = json!({ "type": input });
            let result = sanitize_schema(&schema);
            assert_eq!(
                result["type"], expected,
                "Type {} should convert to {}",
                input, expected
            );
        }
    }

    #[test]
    fn test_removes_conditional_keywords() {
        let schema = json!({
            "type": "object",
            "if": { "properties": { "type": { "const": "a" } } },
            "then": { "properties": { "a_field": { "type": "string" } } },
            "else": { "properties": { "b_field": { "type": "string" } } }
        });

        let result = sanitize_schema(&schema);

        assert!(result.get("if").is_none());
        assert!(result.get("then").is_none());
        assert!(result.get("else").is_none());
    }

    #[test]
    fn test_real_world_anthropic_tool_schema() {
        // A realistic tool schema from Claude Code
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout": {
                    "type": ["integer", "null"],
                    "description": "Timeout in seconds",
                    "minimum": 1,
                    "maximum": 300,
                    "default": 30
                }
            },
            "required": ["command"]
        });

        let result = sanitize_schema(&schema);

        // Verify structure
        assert_eq!(result["type"], "OBJECT");
        assert!(result.get("$schema").is_none());
        assert!(result.get("additionalProperties").is_none());

        // Verify properties
        assert_eq!(result["properties"]["command"]["type"], "STRING");
        assert_eq!(result["properties"]["timeout"]["type"], "INTEGER");
        assert!(result["properties"]["timeout"].get("minimum").is_none());
        assert!(result["properties"]["timeout"].get("maximum").is_none());
        assert!(result["properties"]["timeout"].get("default").is_none());

        // Verify required
        assert_eq!(result["required"], json!(["command"]));
    }
}
