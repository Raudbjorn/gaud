//! Embedded Pricing Database and Cost Calculation
//!
//! Contains per-model pricing data for all supported providers and a simple
//! cost calculator:
//!
//!   cost = (input_tokens / 1_000_000 * input_rate)
//!        + (output_tokens / 1_000_000 * output_rate)

use crate::providers::types::{ModelPricing, TokenUsage};

// ---------------------------------------------------------------------------
// CostDatabase
// ---------------------------------------------------------------------------

/// Embedded, read-only pricing database. Call [`CostDatabase::all()`] to get
/// every known model or [`CostDatabase::for_model()`] to look up a single
/// model by its identifier.
pub struct CostDatabase;

impl CostDatabase {
    /// Calculate cost in USD for the given token usage and pricing.
    pub fn calculate_cost(usage: &TokenUsage, pricing: &ModelPricing) -> f64 {
        let input = usage.input_tokens as f64 / 1_000_000.0 * pricing.input_cost_per_million;
        let output = usage.output_tokens as f64 / 1_000_000.0 * pricing.output_cost_per_million;
        input + output
    }

    /// Look up pricing for a model by its identifier string. Returns `None`
    /// if the model is not in the embedded database.
    pub fn for_model(model: &str) -> Option<ModelPricing> {
        Self::all().into_iter().find(|p| p.model == model)
    }

    /// Return pricing for every known model across all providers.
    pub fn all() -> Vec<ModelPricing> {
        let mut v = Vec::with_capacity(48);
        v.extend(Self::claude_models());
        v.extend(Self::gemini_models());
        v.extend(Self::copilot_models());
        v.extend(Self::openai_models());
        v.extend(Self::deepseek_models());
        v.extend(Self::mistral_models());
        v.extend(Self::kiro_models());
        v
    }

    // -- Claude / Anthropic --------------------------------------------------

    fn claude_models() -> Vec<ModelPricing> {
        vec![
            ModelPricing {
                model: "claude-opus-4-20250514".into(),
                provider: "claude".into(),
                input_cost_per_million: 15.0,
                output_cost_per_million: 75.0,
                context_window: 200_000,
                max_output_tokens: 32_000,
            },
            ModelPricing {
                model: "claude-sonnet-4-20250514".into(),
                provider: "claude".into(),
                input_cost_per_million: 3.0,
                output_cost_per_million: 15.0,
                context_window: 200_000,
                max_output_tokens: 64_000,
            },
            ModelPricing {
                model: "claude-haiku-3-5-20241022".into(),
                provider: "claude".into(),
                input_cost_per_million: 0.80,
                output_cost_per_million: 4.0,
                context_window: 200_000,
                max_output_tokens: 8_192,
            },
            ModelPricing {
                model: "claude-3-5-sonnet-20241022".into(),
                provider: "claude".into(),
                input_cost_per_million: 3.0,
                output_cost_per_million: 15.0,
                context_window: 200_000,
                max_output_tokens: 8_192,
            },
            ModelPricing {
                model: "claude-3-haiku-20240307".into(),
                provider: "claude".into(),
                input_cost_per_million: 0.25,
                output_cost_per_million: 1.25,
                context_window: 200_000,
                max_output_tokens: 4_096,
            },
            ModelPricing {
                model: "claude-3-opus-20240229".into(),
                provider: "claude".into(),
                input_cost_per_million: 15.0,
                output_cost_per_million: 75.0,
                context_window: 200_000,
                max_output_tokens: 4_096,
            },
        ]
    }

    // -- Gemini / Google -----------------------------------------------------

    fn gemini_models() -> Vec<ModelPricing> {
        vec![
            ModelPricing {
                model: "gemini-2.5-pro".into(),
                provider: "gemini".into(),
                input_cost_per_million: 1.25,
                output_cost_per_million: 10.0,
                context_window: 1_000_000,
                max_output_tokens: 65_536,
            },
            ModelPricing {
                model: "gemini-2.5-flash".into(),
                provider: "gemini".into(),
                input_cost_per_million: 0.15,
                output_cost_per_million: 0.60,
                context_window: 1_000_000,
                max_output_tokens: 65_536,
            },
            ModelPricing {
                model: "gemini-2.0-flash".into(),
                provider: "gemini".into(),
                input_cost_per_million: 0.10,
                output_cost_per_million: 0.40,
                context_window: 1_000_000,
                max_output_tokens: 8_192,
            },
            ModelPricing {
                model: "gemini-1.5-pro".into(),
                provider: "gemini".into(),
                input_cost_per_million: 1.25,
                output_cost_per_million: 5.0,
                context_window: 2_000_000,
                max_output_tokens: 8_192,
            },
            ModelPricing {
                model: "gemini-1.5-flash".into(),
                provider: "gemini".into(),
                input_cost_per_million: 0.075,
                output_cost_per_million: 0.30,
                context_window: 1_000_000,
                max_output_tokens: 8_192,
            },
        ]
    }

    // -- Copilot (subscription-based, $0 per-token) --------------------------

    fn copilot_models() -> Vec<ModelPricing> {
        vec![
            ModelPricing {
                model: "gpt-4o".into(),
                provider: "copilot".into(),
                input_cost_per_million: 0.0,
                output_cost_per_million: 0.0,
                context_window: 128_000,
                max_output_tokens: 16_384,
            },
            ModelPricing {
                model: "gpt-4-turbo".into(),
                provider: "copilot".into(),
                input_cost_per_million: 0.0,
                output_cost_per_million: 0.0,
                context_window: 128_000,
                max_output_tokens: 4_096,
            },
            ModelPricing {
                model: "o1".into(),
                provider: "copilot".into(),
                input_cost_per_million: 0.0,
                output_cost_per_million: 0.0,
                context_window: 200_000,
                max_output_tokens: 100_000,
            },
            ModelPricing {
                model: "o3-mini".into(),
                provider: "copilot".into(),
                input_cost_per_million: 0.0,
                output_cost_per_million: 0.0,
                context_window: 200_000,
                max_output_tokens: 100_000,
            },
        ]
    }

    // -- OpenAI (direct, not via Copilot) ------------------------------------

    fn openai_models() -> Vec<ModelPricing> {
        vec![
            ModelPricing {
                model: "gpt-4o-direct".into(),
                provider: "openai".into(),
                input_cost_per_million: 2.50,
                output_cost_per_million: 10.0,
                context_window: 128_000,
                max_output_tokens: 16_384,
            },
            ModelPricing {
                model: "gpt-4o-mini".into(),
                provider: "openai".into(),
                input_cost_per_million: 0.15,
                output_cost_per_million: 0.60,
                context_window: 128_000,
                max_output_tokens: 16_384,
            },
            ModelPricing {
                model: "gpt-4-turbo-direct".into(),
                provider: "openai".into(),
                input_cost_per_million: 10.0,
                output_cost_per_million: 30.0,
                context_window: 128_000,
                max_output_tokens: 4_096,
            },
            ModelPricing {
                model: "o1-preview".into(),
                provider: "openai".into(),
                input_cost_per_million: 15.0,
                output_cost_per_million: 60.0,
                context_window: 128_000,
                max_output_tokens: 32_768,
            },
            ModelPricing {
                model: "o1-mini".into(),
                provider: "openai".into(),
                input_cost_per_million: 3.0,
                output_cost_per_million: 12.0,
                context_window: 128_000,
                max_output_tokens: 65_536,
            },
            ModelPricing {
                model: "o1-direct".into(),
                provider: "openai".into(),
                input_cost_per_million: 15.0,
                output_cost_per_million: 60.0,
                context_window: 200_000,
                max_output_tokens: 100_000,
            },
            ModelPricing {
                model: "o3-mini-direct".into(),
                provider: "openai".into(),
                input_cost_per_million: 1.10,
                output_cost_per_million: 4.40,
                context_window: 200_000,
                max_output_tokens: 100_000,
            },
            ModelPricing {
                model: "gpt-3.5-turbo".into(),
                provider: "openai".into(),
                input_cost_per_million: 0.50,
                output_cost_per_million: 1.50,
                context_window: 16_385,
                max_output_tokens: 4_096,
            },
        ]
    }

    // -- DeepSeek ------------------------------------------------------------

    fn deepseek_models() -> Vec<ModelPricing> {
        vec![
            ModelPricing {
                model: "deepseek-chat".into(),
                provider: "deepseek".into(),
                input_cost_per_million: 0.14,
                output_cost_per_million: 0.28,
                context_window: 64_000,
                max_output_tokens: 4_096,
            },
            ModelPricing {
                model: "deepseek-coder".into(),
                provider: "deepseek".into(),
                input_cost_per_million: 0.14,
                output_cost_per_million: 0.28,
                context_window: 128_000,
                max_output_tokens: 4_096,
            },
            ModelPricing {
                model: "deepseek-reasoner".into(),
                provider: "deepseek".into(),
                input_cost_per_million: 0.55,
                output_cost_per_million: 2.19,
                context_window: 64_000,
                max_output_tokens: 8_192,
            },
        ]
    }

    // -- Kiro (subscription-based, $0 per-token) ----------------------------

    fn kiro_models() -> Vec<ModelPricing> {
        vec![
            ModelPricing {
                model: "kiro:auto".into(),
                provider: "kiro".into(),
                input_cost_per_million: 0.0,
                output_cost_per_million: 0.0,
                context_window: 200_000,
                max_output_tokens: 64_000,
            },
            ModelPricing {
                model: "kiro:claude-sonnet-4".into(),
                provider: "kiro".into(),
                input_cost_per_million: 0.0,
                output_cost_per_million: 0.0,
                context_window: 200_000,
                max_output_tokens: 64_000,
            },
            ModelPricing {
                model: "kiro:claude-sonnet-4.5".into(),
                provider: "kiro".into(),
                input_cost_per_million: 0.0,
                output_cost_per_million: 0.0,
                context_window: 200_000,
                max_output_tokens: 64_000,
            },
            ModelPricing {
                model: "kiro:claude-haiku-4.5".into(),
                provider: "kiro".into(),
                input_cost_per_million: 0.0,
                output_cost_per_million: 0.0,
                context_window: 200_000,
                max_output_tokens: 8_192,
            },
            ModelPricing {
                model: "kiro:claude-opus-4.5".into(),
                provider: "kiro".into(),
                input_cost_per_million: 0.0,
                output_cost_per_million: 0.0,
                context_window: 200_000,
                max_output_tokens: 32_000,
            },
            ModelPricing {
                model: "kiro:claude-3.7-sonnet".into(),
                provider: "kiro".into(),
                input_cost_per_million: 0.0,
                output_cost_per_million: 0.0,
                context_window: 200_000,
                max_output_tokens: 8_192,
            },
        ]
    }

    // -- Mistral -------------------------------------------------------------

    fn mistral_models() -> Vec<ModelPricing> {
        vec![
            ModelPricing {
                model: "mistral-large".into(),
                provider: "mistral".into(),
                input_cost_per_million: 2.0,
                output_cost_per_million: 6.0,
                context_window: 128_000,
                max_output_tokens: 128_000,
            },
            ModelPricing {
                model: "mistral-medium".into(),
                provider: "mistral".into(),
                input_cost_per_million: 2.7,
                output_cost_per_million: 8.1,
                context_window: 32_000,
                max_output_tokens: 32_000,
            },
            ModelPricing {
                model: "mistral-small".into(),
                provider: "mistral".into(),
                input_cost_per_million: 0.2,
                output_cost_per_million: 0.6,
                context_window: 32_000,
                max_output_tokens: 32_000,
            },
            ModelPricing {
                model: "codestral".into(),
                provider: "mistral".into(),
                input_cost_per_million: 0.2,
                output_cost_per_million: 0.6,
                context_window: 32_000,
                max_output_tokens: 32_000,
            },
            ModelPricing {
                model: "mistral-nemo".into(),
                provider: "mistral".into(),
                input_cost_per_million: 0.15,
                output_cost_per_million: 0.15,
                context_window: 128_000,
                max_output_tokens: 128_000,
            },
        ]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_cost_basic() {
        let usage = TokenUsage {
            input_tokens: 1_000,
            output_tokens: 500,
        };
        let pricing = ModelPricing {
            model: "test".into(),
            provider: "test".into(),
            input_cost_per_million: 3.0,
            output_cost_per_million: 15.0,
            context_window: 200_000,
            max_output_tokens: 8_192,
        };
        // (1000/1M * 3.0) + (500/1M * 15.0) = 0.003 + 0.0075 = 0.0105
        let cost = CostDatabase::calculate_cost(&usage, &pricing);
        assert!((cost - 0.0105).abs() < 0.0001);
    }

    #[test]
    fn test_calculate_cost_zero_tokens() {
        let usage = TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
        };
        let pricing = ModelPricing {
            model: "test".into(),
            provider: "test".into(),
            input_cost_per_million: 3.0,
            output_cost_per_million: 15.0,
            context_window: 200_000,
            max_output_tokens: 8_192,
        };
        assert_eq!(CostDatabase::calculate_cost(&usage, &pricing), 0.0);
    }

    #[test]
    fn test_calculate_cost_free_model() {
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
        };
        let pricing = ModelPricing {
            model: "gpt-4o".into(),
            provider: "copilot".into(),
            input_cost_per_million: 0.0,
            output_cost_per_million: 0.0,
            context_window: 128_000,
            max_output_tokens: 16_384,
        };
        assert_eq!(CostDatabase::calculate_cost(&usage, &pricing), 0.0);
    }

    #[test]
    fn test_for_model_found() {
        let pricing = CostDatabase::for_model("claude-sonnet-4-20250514");
        assert!(pricing.is_some());
        let p = pricing.unwrap();
        assert_eq!(p.provider, "claude");
        assert_eq!(p.input_cost_per_million, 3.0);
        assert_eq!(p.output_cost_per_million, 15.0);
    }

    #[test]
    fn test_for_model_not_found() {
        assert!(CostDatabase::for_model("nonexistent-model-xyz").is_none());
    }

    #[test]
    fn test_all_returns_many_models() {
        let all = CostDatabase::all();
        assert!(all.len() >= 30, "Expected 30+ models, got {}", all.len());
    }

    #[test]
    fn test_gemini_models_present() {
        let p = CostDatabase::for_model("gemini-2.5-flash");
        assert!(p.is_some());
        let p = p.unwrap();
        assert_eq!(p.provider, "gemini");
        assert_eq!(p.input_cost_per_million, 0.15);
    }

    #[test]
    fn test_copilot_models_free() {
        let p = CostDatabase::for_model("gpt-4o").unwrap();
        assert_eq!(p.provider, "copilot");
        assert_eq!(p.input_cost_per_million, 0.0);
        assert_eq!(p.output_cost_per_million, 0.0);
    }

    #[test]
    fn test_deepseek_models_present() {
        let p = CostDatabase::for_model("deepseek-chat").unwrap();
        assert_eq!(p.provider, "deepseek");
        assert_eq!(p.input_cost_per_million, 0.14);
    }

    #[test]
    fn test_mistral_models_present() {
        let p = CostDatabase::for_model("mistral-large").unwrap();
        assert_eq!(p.provider, "mistral");
        assert_eq!(p.input_cost_per_million, 2.0);
    }

    #[test]
    fn test_claude_opus_pricing() {
        let p = CostDatabase::for_model("claude-opus-4-20250514").unwrap();
        assert_eq!(p.input_cost_per_million, 15.0);
        assert_eq!(p.output_cost_per_million, 75.0);
        assert_eq!(p.context_window, 200_000);
    }

    #[test]
    fn test_kiro_models_free() {
        let p = CostDatabase::for_model("kiro:auto").unwrap();
        assert_eq!(p.provider, "kiro");
        assert_eq!(p.input_cost_per_million, 0.0);
        assert_eq!(p.output_cost_per_million, 0.0);
    }

    #[test]
    fn test_kiro_sonnet_present() {
        let p = CostDatabase::for_model("kiro:claude-sonnet-4").unwrap();
        assert_eq!(p.provider, "kiro");
        assert_eq!(p.input_cost_per_million, 0.0);
    }

    #[test]
    fn test_large_token_cost() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
        };
        let pricing = CostDatabase::for_model("claude-sonnet-4-20250514").unwrap();
        // (1M/1M * 3.0) + (500K/1M * 15.0) = 3.0 + 7.5 = 10.5
        let cost = CostDatabase::calculate_cost(&usage, &pricing);
        assert!((cost - 10.5).abs() < 0.0001);
    }
}
