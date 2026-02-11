//! Cost calculation for LLM API requests.
//!
//! Calculates the cost of API requests based on token usage and model pricing.
//! Supports both token-based and cached token pricing.

use super::pricing::{ModelPricing, PricingDatabase};
use super::types::Usage;
#[cfg(test)]
use super::types::UsageTokenDetails;
use anyhow::{Context, Result};
use tracing::{debug, warn};

// MARK: - Cost Calculator

/// Calculator for LLM API request costs.
pub struct CostCalculator {
    pricing_db: PricingDatabase,
}

impl CostCalculator {
    /// Create a new cost calculator with the default pricing database.
    pub fn new() -> Self {
        Self {
            pricing_db: PricingDatabase::new(),
        }
    }

    /// Create a cost calculator with a custom pricing database.
    pub fn with_pricing_db(pricing_db: PricingDatabase) -> Self {
        Self { pricing_db }
    }

    /// Calculate the cost of a request based on usage and model.
    ///
    /// Returns the cost in USD. If pricing is not available for the model,
    /// returns 0.0 and logs a warning.
    pub fn calculate_cost(&self, model: &str, usage: &Usage) -> f64 {
        match self.try_calculate_cost(model, usage) {
            Ok(cost) => cost,
            Err(e) => {
                warn!(
                    model = %model,
                    error = %e,
                    "Failed to calculate cost, returning 0.0"
                );
                0.0
            }
        }
    }

    /// Calculate the cost of a request, returning an error if pricing is unavailable.
    pub fn try_calculate_cost(&self, model: &str, usage: &Usage) -> Result<f64> {
        let pricing = self
            .pricing_db
            .get(model)
            .context(format!("No pricing data for model: {}", model))?;

        let cost = self.calculate_cost_with_pricing(pricing, usage);

        debug!(
            model = %model,
            input_tokens = usage.prompt_tokens,
            output_tokens = usage.completion_tokens,
            cost_usd = %format!("${:.6}", cost),
            "Calculated request cost"
        );

        Ok(cost)
    }

    /// Calculate cost using specific pricing information.
    fn calculate_cost_with_pricing(&self, pricing: &ModelPricing, usage: &Usage) -> f64 {
        // Calculate input cost (regular + cached if available)
        let cached_tokens = usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens);

        let input_cost = if let Some(cached) = cached_tokens {
            let regular_tokens = usage.prompt_tokens.saturating_sub(cached);
            let regular_cost = (regular_tokens as f64 / 1_000_000.0)
                * pricing.input_cost_per_million;

            let cached_cost = if let Some(cached_price) = pricing.cached_input_cost_per_million {
                (cached as f64 / 1_000_000.0) * cached_price
            } else {
                // If no cached pricing, use regular pricing
                (cached as f64 / 1_000_000.0) * pricing.input_cost_per_million
            };

            regular_cost + cached_cost
        } else {
            (usage.prompt_tokens as f64 / 1_000_000.0) * pricing.input_cost_per_million
        };

        // Calculate output cost
        let output_cost =
            (usage.completion_tokens as f64 / 1_000_000.0) * pricing.output_cost_per_million;

        input_cost + output_cost
    }

    /// Get pricing information for a model.
    pub fn get_pricing(&self, model: &str) -> Option<&ModelPricing> {
        self.pricing_db.get(model)
    }

    /// Check if pricing is available for a model.
    pub fn has_pricing(&self, model: &str) -> bool {
        self.pricing_db.has_pricing(model)
    }

    /// Get all available pricing information.
    pub fn all_pricing(&self) -> Vec<&ModelPricing> {
        self.pricing_db.all()
    }

    /// Get all pricing information (static method for backward compatibility).
    pub fn all() -> Vec<ModelPricing> {
        PricingDatabase::new().all().into_iter().cloned().collect()
    }
}

impl Default for CostCalculator {
    fn default() -> Self {
        Self::new()
    }
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_cost_claude() {
        let calculator = CostCalculator::new();

        let usage = Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };

        let cost = calculator.calculate_cost("claude-sonnet-4-20250514", &usage);

        // Expected: (1000/1M * $3) + (500/1M * $15) = $0.003 + $0.0075 = $0.0105
        assert!((cost - 0.0105).abs() < 0.0001);
    }

    #[test]
    fn test_calculate_cost_with_caching() {
        let calculator = CostCalculator::new();

        let usage = Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            prompt_tokens_details: Some(UsageTokenDetails {
                cached_tokens: Some(800), // 800 cached, 200 regular
                reasoning_tokens: None,
            }),
            completion_tokens_details: None,
        };

        let cost = calculator.calculate_cost("claude-sonnet-4-20250514", &usage);

        // Expected:
        // Regular input: (200/1M * $3) = $0.0006
        // Cached input: (800/1M * $0.30) = $0.00024
        // Output: (500/1M * $15) = $0.0075
        // Total: $0.00834
        assert!((cost - 0.00834).abs() < 0.0001);
    }

    #[test]
    fn test_calculate_cost_gemini() {
        let calculator = CostCalculator::new();

        let usage = Usage {
            prompt_tokens: 10000,
            completion_tokens: 2000,
            total_tokens: 12000,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };

        let cost = calculator.calculate_cost("gemini-2.5-flash", &usage);

        // Expected: (10000/1M * $0.075) + (2000/1M * $0.30) = $0.00075 + $0.0006 = $0.00135
        assert!((cost - 0.00135).abs() < 0.0001);
    }

    #[test]
    fn test_calculate_cost_copilot() {
        let calculator = CostCalculator::new();

        let usage = Usage {
            prompt_tokens: 5000,
            completion_tokens: 1000,
            total_tokens: 6000,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };

        let cost = calculator.calculate_cost("gpt-4o", &usage);

        // Expected: (5000/1M * $2.50) + (1000/1M * $10.00) = $0.0125 + $0.01 = $0.0225
        assert!((cost - 0.0225).abs() < 0.0001);
    }

    #[test]
    fn test_calculate_cost_unknown_model() {
        let calculator = CostCalculator::new();

        let usage = Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };

        // Should return 0.0 for unknown models
        let cost = calculator.calculate_cost("unknown-model", &usage);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_has_pricing() {
        let calculator = CostCalculator::new();
        assert!(calculator.has_pricing("claude-sonnet-4-20250514"));
        assert!(calculator.has_pricing("gemini-2.5-flash"));
        assert!(calculator.has_pricing("gpt-4o"));
        assert!(!calculator.has_pricing("unknown-model"));
    }
}

// MARK: - Property-Based Tests

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    /// Generate arbitrary Usage instances for property testing.
    fn usage_strategy() -> impl Strategy<Value = Usage> {
        (
            0u32..1_000_000,  // prompt_tokens (0 to 1M)
            0u32..1_000_000,  // completion_tokens (0 to 1M)
            prop::option::of(0u32..1_000_000),  // cached_tokens (optional)
        )
            .prop_map(|(prompt_tokens, completion_tokens, cached_tokens)| {
                let total_tokens = prompt_tokens + completion_tokens;
                let prompt_tokens_details = cached_tokens.map(|cached| {
                    // Ensure cached tokens don't exceed prompt tokens
                    let cached = cached.min(prompt_tokens);
                    UsageTokenDetails {
                        cached_tokens: Some(cached),
                        reasoning_tokens: None,
                    }
                });

                Usage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens,
                    prompt_tokens_details,
                    completion_tokens_details: None,
                }
            })
    }

    /// Generate model names that have pricing data.
    fn known_model_strategy() -> impl Strategy<Value = String> {
        prop::sample::select(vec![
            "claude-sonnet-4-20250514".to_string(),
            "claude-opus-4-20250514".to_string(),
            "claude-haiku-3-5-20241022".to_string(),
            "gemini-2.5-flash".to_string(),
            "gemini-2.5-pro".to_string(),
            "gemini-2.0-flash".to_string(),
            "gpt-4o".to_string(),
            "gpt-4-turbo".to_string(),
            "o1".to_string(),
            "o3-mini".to_string(),
        ])
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// **Validates: Requirements 2.1, 2.2, 2.5**
        ///
        /// Feature: litellm-integration-completion, Property 1: Cost Calculation for Successful Requests
        ///
        /// For any successful chat request (streaming or non-streaming), the audit entry should contain
        /// a calculated cost greater than or equal to 0.0, where the cost is computed from the usage
        /// information and model pricing.
        ///
        /// This property verifies that:
        /// 1. Cost is always non-negative for known models
        /// 2. Cost is proportional to token usage (more tokens = higher cost)
        /// 3. Cost calculation never panics or returns invalid values
        #[test]
        fn prop_cost_calculation_for_successful_requests(
            model in known_model_strategy(),
            usage in usage_strategy()
        ) {
            let calculator = CostCalculator::new();

            // Calculate cost for the given model and usage
            let cost = calculator.calculate_cost(&model, &usage);

            // Property 1: Cost must be non-negative
            prop_assert!(cost >= 0.0, "Cost must be non-negative, got: {}", cost);

            // Property 2: Cost must be finite (not NaN or infinity)
            prop_assert!(cost.is_finite(), "Cost must be finite, got: {}", cost);

            // Property 3: If there are tokens, cost should be positive (for known models)
            if usage.prompt_tokens > 0 || usage.completion_tokens > 0 {
                prop_assert!(cost > 0.0, "Cost should be positive when tokens are used, got: {}", cost);
            }

            // Property 4: Cost should be proportional to token count
            // More tokens should never result in less cost
            if usage.prompt_tokens > 0 || usage.completion_tokens > 0 {
                let double_usage = Usage {
                    prompt_tokens: usage.prompt_tokens * 2,
                    completion_tokens: usage.completion_tokens * 2,
                    total_tokens: usage.total_tokens * 2,
                    prompt_tokens_details: usage.prompt_tokens_details.as_ref().map(|details| {
                        UsageTokenDetails {
                            cached_tokens: details.cached_tokens.map(|c| c * 2),
                            reasoning_tokens: details.reasoning_tokens,
                        }
                    }),
                    completion_tokens_details: None,
                };
                let double_cost = calculator.calculate_cost(&model, &double_usage);

                // Double the tokens should result in approximately double the cost
                // Allow for small floating point errors
                let ratio = double_cost / cost;
                prop_assert!(
                    (ratio - 2.0).abs() < 0.01,
                    "Double tokens should result in ~double cost. Original: {}, Double: {}, Ratio: {}",
                    cost, double_cost, ratio
                );
            }

            // Property 5: Zero tokens should result in zero cost
            let zero_usage = Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            };
            let zero_cost = calculator.calculate_cost(&model, &zero_usage);
            prop_assert_eq!(zero_cost, 0.0, "Zero tokens should result in zero cost");
        }

        /// **Validates: Requirements 2.1, 2.2, 2.5**
        ///
        /// Additional property: Cost calculation should be consistent and deterministic.
        /// Calling calculate_cost multiple times with the same inputs should always
        /// return the same result.
        #[test]
        fn prop_cost_calculation_is_deterministic(
            model in known_model_strategy(),
            usage in usage_strategy()
        ) {
            let calculator = CostCalculator::new();

            let cost1 = calculator.calculate_cost(&model, &usage);
            let cost2 = calculator.calculate_cost(&model, &usage);
            let cost3 = calculator.calculate_cost(&model, &usage);

            prop_assert_eq!(cost1, cost2, "Cost calculation should be deterministic");
            prop_assert_eq!(cost2, cost3, "Cost calculation should be deterministic");
        }

        /// **Validates: Requirements 2.3**
        ///
        /// Property: Unknown models should return 0.0 cost without panicking.
        #[test]
        fn prop_unknown_model_returns_zero(
            unknown_model in "[a-z]{5,15}-[0-9]{1,3}",
            usage in usage_strategy()
        ) {
            let calculator = CostCalculator::new();

            // Skip if the random model happens to be a known one
            if calculator.has_pricing(&unknown_model) {
                return Ok(());
            }

            let cost = calculator.calculate_cost(&unknown_model, &usage);

            prop_assert_eq!(cost, 0.0, "Unknown model should return 0.0 cost");
            prop_assert!(cost.is_finite(), "Cost should be finite even for unknown models");
        }
    }
}
