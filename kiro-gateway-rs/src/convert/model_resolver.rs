//! Model name normalization and resolution.
//!
//! Translates Anthropic-style model names to Kiro model IDs.
//! Examples:
//! - "claude-sonnet-4-5" -> "claude-sonnet-4.5"
//! - "claude-sonnet-4-5-20250514" -> "claude-sonnet-4.5"
//! - "claude-3-5-sonnet-20241022" -> "claude-3.5-sonnet"
//! - "claude-3-5-sonnet-v2" -> "claude-3.5-sonnet"

use std::collections::HashMap;
use std::sync::RwLock;
use tracing::debug;

/// Normalize a model name by applying Kiro's naming rules.
///
/// 1. Replace dashes between version numbers with dots (e.g., `4-5` -> `4.5`)
/// 2. Strip date suffixes (e.g., `-20250514`)
/// 3. Strip version suffixes (e.g., `-v2`)
/// 4. Convert legacy `claude-3-X-name` to `claude-3.X-name`
pub fn normalize_model_name(name: &str) -> String {
    let mut result = name.to_string();

    // Strip date suffix first: "-20241022", "-20250514" (8 digits at end)
    let re_date = regex_lite::Regex::new(r"-\d{8}$").unwrap();
    result = re_date.replace(&result, "").to_string();

    // Strip version suffix: "-v1", "-v2"
    let re_vsuffix = regex_lite::Regex::new(r"-v\d+$").unwrap();
    result = re_vsuffix.replace(&result, "").to_string();

    // Replace dash between adjacent single digits: "4-5" -> "4.5", "3-5" -> "3.5"
    // regex-lite doesn't support lookahead, so use a simple pattern:
    // Match digit-dash-digit and replace with digit.digit
    let re_version_dash = regex_lite::Regex::new(r"(\d)-(\d)").unwrap();
    result = re_version_dash.replace_all(&result, "${1}.${2}").to_string();

    result
}

/// Model resolver with caching and alias support.
pub struct ModelResolver {
    /// Direct aliases (e.g., "sonnet" -> "claude-sonnet-4.5").
    aliases: HashMap<String, String>,
    /// Cached model ID lookups.
    cache: RwLock<HashMap<String, String>>,
    /// Available models from Kiro API (model_id values).
    available_models: RwLock<Vec<String>>,
}

impl ModelResolver {
    /// Create a new model resolver.
    pub fn new() -> Self {
        Self {
            aliases: Self::default_aliases(),
            cache: RwLock::new(HashMap::new()),
            available_models: RwLock::new(Vec::new()),
        }
    }

    /// Set the list of available models (from ListAvailableModels).
    pub fn set_available_models(&self, models: Vec<String>) {
        let mut available = self.available_models.write().unwrap();
        *available = models;

        // Clear cache when models list changes
        let mut cache = self.cache.write().unwrap();
        cache.clear();
    }

    /// Resolve a model name to a Kiro model ID.
    ///
    /// Resolution order:
    /// 1. Check direct aliases
    /// 2. Normalize the name
    /// 3. Check cache
    /// 4. Match against available models
    /// 5. Check hidden models
    /// 6. Pass through as-is
    pub fn resolve(&self, name: &str) -> String {
        // 1. Direct alias
        if let Some(alias) = self.aliases.get(name) {
            debug!(name, resolved = alias.as_str(), "Model alias matched");
            return alias.clone();
        }

        // 2. Normalize
        let normalized = normalize_model_name(name);

        // 3. Cache lookup
        {
            let cache = self.cache.read().unwrap();
            if let Some(cached) = cache.get(&normalized) {
                return cached.clone();
            }
        }

        // 4. Match against available models
        let available = self.available_models.read().unwrap();
        for model_id in available.iter() {
            let norm_available = normalize_model_name(model_id);
            if norm_available == normalized {
                let mut cache = self.cache.write().unwrap();
                cache.insert(normalized, model_id.clone());
                debug!(name, resolved = model_id.as_str(), "Model matched from available");
                return model_id.clone();
            }
        }

        // 5. Check hidden models
        for (hidden_name, hidden_id) in crate::config::hidden_models() {
            if normalized == hidden_name || name == hidden_id {
                let result = hidden_id.to_string();
                let mut cache = self.cache.write().unwrap();
                cache.insert(normalized, result.clone());
                debug!(name, resolved = result.as_str(), "Hidden model matched");
                return result;
            }
        }

        // 6. Pass through
        debug!(name, "Model passed through unchanged");
        normalized
    }

    fn default_aliases() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("auto".into(), "auto".into());
        m.insert("sonnet".into(), "claude-sonnet-4".into());
        m.insert("haiku".into(), "claude-haiku-4.5".into());
        m.insert("opus".into(), "claude-opus-4.5".into());
        m
    }
}

impl Default for ModelResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_version_dash() {
        assert_eq!(normalize_model_name("claude-sonnet-4-5"), "claude-sonnet-4.5");
        assert_eq!(normalize_model_name("claude-haiku-4-5"), "claude-haiku-4.5");
    }

    #[test]
    fn test_normalize_strip_date() {
        assert_eq!(
            normalize_model_name("claude-sonnet-4-5-20250514"),
            "claude-sonnet-4.5"
        );
        assert_eq!(
            normalize_model_name("claude-3-5-sonnet-20241022"),
            "claude-3.5-sonnet"
        );
    }

    #[test]
    fn test_normalize_strip_version_suffix() {
        assert_eq!(
            normalize_model_name("claude-3-5-sonnet-v2"),
            "claude-3.5-sonnet"
        );
    }

    #[test]
    fn test_normalize_passthrough() {
        assert_eq!(normalize_model_name("auto"), "auto");
        assert_eq!(
            normalize_model_name("claude-sonnet-4.5"),
            "claude-sonnet-4.5"
        );
    }

    #[test]
    fn test_resolver_aliases() {
        let resolver = ModelResolver::new();
        assert_eq!(resolver.resolve("auto"), "auto");
        assert_eq!(resolver.resolve("sonnet"), "claude-sonnet-4");
    }

    #[test]
    fn test_resolver_normalizes() {
        let resolver = ModelResolver::new();
        assert_eq!(
            resolver.resolve("claude-sonnet-4-5-20250514"),
            "claude-sonnet-4.5"
        );
    }
}
