//! Model pricing database.
//!
//! Contains pricing information for all supported models across providers.
//! Prices are in USD per 1M tokens (input/output).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// MARK: - Types

/// Pricing information for a specific model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Model identifier (e.g., "claude-sonnet-4-20250514").
    pub model: String,
    /// Provider identifier (e.g., "claude", "gemini", "copilot").
    pub provider: String,
    /// Cost per 1M input tokens in USD.
    pub input_cost_per_million: f64,
    /// Cost per 1M output tokens in USD.
    pub output_cost_per_million: f64,
    /// Optional: Cost per 1M cached input tokens (for providers with caching).
    pub cached_input_cost_per_million: Option<f64>,
}

/// Pricing database containing all model pricing information.
#[derive(Debug, Clone)]
pub struct PricingDatabase {
    /// Model name -> pricing info.
    pricing: HashMap<String, ModelPricing>,
}

// MARK: - Implementation

impl PricingDatabase {
    /// Create a new pricing database with default pricing data.
    pub fn new() -> Self {
        let mut pricing = HashMap::new();

        // Claude (Anthropic) pricing
        // Source: https://www.anthropic.com/pricing
        pricing.insert(
            "claude-sonnet-4-20250514".to_string(),
            ModelPricing {
                model: "claude-sonnet-4-20250514".to_string(),
                provider: "claude".to_string(),
                input_cost_per_million: 3.00,
                output_cost_per_million: 15.00,
                cached_input_cost_per_million: Some(0.30),
            },
        );
        pricing.insert(
            "claude-opus-4-20250514".to_string(),
            ModelPricing {
                model: "claude-opus-4-20250514".to_string(),
                provider: "claude".to_string(),
                input_cost_per_million: 15.00,
                output_cost_per_million: 75.00,
                cached_input_cost_per_million: Some(1.50),
            },
        );
        pricing.insert(
            "claude-haiku-3-5-20241022".to_string(),
            ModelPricing {
                model: "claude-haiku-3-5-20241022".to_string(),
                provider: "claude".to_string(),
                input_cost_per_million: 1.00,
                output_cost_per_million: 5.00,
                cached_input_cost_per_million: Some(0.10),
            },
        );

        // Gemini (Google) pricing
        // Source: https://ai.google.dev/pricing
        pricing.insert(
            "gemini-2.5-flash".to_string(),
            ModelPricing {
                model: "gemini-2.5-flash".to_string(),
                provider: "gemini".to_string(),
                input_cost_per_million: 0.075,
                output_cost_per_million: 0.30,
                cached_input_cost_per_million: Some(0.01875),
            },
        );
        pricing.insert(
            "gemini-2.5-pro".to_string(),
            ModelPricing {
                model: "gemini-2.5-pro".to_string(),
                provider: "gemini".to_string(),
                input_cost_per_million: 1.25,
                output_cost_per_million: 5.00,
                cached_input_cost_per_million: Some(0.3125),
            },
        );
        pricing.insert(
            "gemini-2.0-flash".to_string(),
            ModelPricing {
                model: "gemini-2.0-flash".to_string(),
                provider: "gemini".to_string(),
                input_cost_per_million: 0.075,
                output_cost_per_million: 0.30,
                cached_input_cost_per_million: Some(0.01875),
            },
        );

        // GitHub Copilot pricing
        // Note: Copilot uses OpenAI models with GitHub's pricing
        pricing.insert(
            "gpt-4o".to_string(),
            ModelPricing {
                model: "gpt-4o".to_string(),
                provider: "copilot".to_string(),
                input_cost_per_million: 2.50,
                output_cost_per_million: 10.00,
                cached_input_cost_per_million: Some(1.25),
            },
        );
        pricing.insert(
            "gpt-4-turbo".to_string(),
            ModelPricing {
                model: "gpt-4-turbo".to_string(),
                provider: "copilot".to_string(),
                input_cost_per_million: 10.00,
                output_cost_per_million: 30.00,
                cached_input_cost_per_million: None,
            },
        );
        pricing.insert(
            "o1".to_string(),
            ModelPricing {
                model: "o1".to_string(),
                provider: "copilot".to_string(),
                input_cost_per_million: 15.00,
                output_cost_per_million: 60.00,
                cached_input_cost_per_million: Some(7.50),
            },
        );
        pricing.insert(
            "o3-mini".to_string(),
            ModelPricing {
                model: "o3-mini".to_string(),
                provider: "copilot".to_string(),
                input_cost_per_million: 1.10,
                output_cost_per_million: 4.40,
                cached_input_cost_per_million: Some(0.55),
            },
        );

        Self { pricing }
    }

    /// Get pricing for a specific model.
    pub fn get(&self, model: &str) -> Option<&ModelPricing> {
        self.pricing.get(model)
    }

    /// Get all pricing information.
    pub fn all(&self) -> Vec<&ModelPricing> {
        self.pricing.values().collect()
    }

    /// Check if pricing exists for a model.
    pub fn has_pricing(&self, model: &str) -> bool {
        self.pricing.contains_key(model)
    }
}

impl Default for PricingDatabase {
    fn default() -> Self {
        Self::new()
    }
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pricing_database() {
        let db = PricingDatabase::new();

        // Test Claude pricing
        let claude = db.get("claude-sonnet-4-20250514").unwrap();
        assert_eq!(claude.provider, "claude");
        assert_eq!(claude.input_cost_per_million, 3.00);
        assert_eq!(claude.output_cost_per_million, 15.00);

        // Test Gemini pricing
        let gemini = db.get("gemini-2.5-flash").unwrap();
        assert_eq!(gemini.provider, "gemini");
        assert_eq!(gemini.input_cost_per_million, 0.075);

        // Test Copilot pricing
        let copilot = db.get("gpt-4o").unwrap();
        assert_eq!(copilot.provider, "copilot");
        assert_eq!(copilot.input_cost_per_million, 2.50);

        // Test missing model
        assert!(db.get("nonexistent-model").is_none());
    }

    #[test]
    fn test_has_pricing() {
        let db = PricingDatabase::new();
        assert!(db.has_pricing("claude-sonnet-4-20250514"));
        assert!(db.has_pricing("gemini-2.5-flash"));
        assert!(db.has_pricing("gpt-4o"));
        assert!(!db.has_pricing("unknown-model"));
    }
}
