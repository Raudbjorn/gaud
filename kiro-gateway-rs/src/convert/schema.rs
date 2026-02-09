//! JSON Schema sanitization for Kiro API compatibility.
//!
//! Kiro's API is stricter about JSON Schema than Anthropic's.
//! This module cleans up schemas to avoid validation errors.

use serde_json::Value;

/// Sanitize a JSON Schema for Kiro compatibility.
///
/// Removes:
/// - Empty `required` arrays
/// - `additionalProperties` at the top level
/// - `$schema` field
/// - Nested schemas are recursively sanitized
pub fn sanitize_json_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut result = serde_json::Map::new();
            for (key, value) in map {
                match key.as_str() {
                    // Remove empty required arrays
                    "required" => {
                        if let Value::Array(arr) = value {
                            if !arr.is_empty() {
                                result.insert(key.clone(), value.clone());
                            }
                        }
                    }
                    // Remove additionalProperties
                    "additionalProperties" | "$schema" => {
                        // Skip
                    }
                    // Recursively sanitize all nested objects/arrays
                    _ => {
                        result.insert(key.clone(), sanitize_json_schema(value));
                    }
                }
            }
            Value::Object(result)
        }
        Value::Array(arr) => {
            Value::Array(arr.iter().map(sanitize_json_schema).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_remove_empty_required() {
        let schema = json!({
            "type": "object",
            "properties": {},
            "required": []
        });
        let result = sanitize_json_schema(&schema);
        assert!(result.get("required").is_none());
    }

    #[test]
    fn test_keep_nonempty_required() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        });
        let result = sanitize_json_schema(&schema);
        assert!(result.get("required").is_some());
    }

    #[test]
    fn test_remove_additional_properties() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false
        });
        let result = sanitize_json_schema(&schema);
        assert!(result.get("additionalProperties").is_none());
    }

    #[test]
    fn test_recursive_sanitization() {
        let schema = json!({
            "type": "object",
            "properties": {
                "inner": {
                    "type": "object",
                    "required": [],
                    "additionalProperties": false
                }
            }
        });
        let result = sanitize_json_schema(&schema);
        let inner = result
            .get("properties")
            .unwrap()
            .get("inner")
            .unwrap();
        assert!(inner.get("required").is_none());
        assert!(inner.get("additionalProperties").is_none());
    }
}
