//! Provider traits.

use std::future::Future;
use std::pin::Pin;

use futures::Stream;

use crate::providers::ProviderError;
use crate::providers::pricing::ModelPricing;
use crate::providers::types::{ChatChunk, ChatRequest, ChatResponse};

/// Trait that all LLM providers must implement.
///
/// Async methods return boxed futures so the trait is dyn-compatible (can be
/// used as `Arc<dyn LlmProvider>`). No `async_trait` macro is needed.
pub trait LlmProvider: Send + Sync {
    /// Unique identifier for this provider (e.g. "claude", "gemini", "copilot").
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn name(&self) -> &str;

    /// List of model identifiers this provider supports.
    fn models(&self) -> Vec<String>;

    /// Check whether a specific model string is handled by this provider.
    fn supports_model(&self, model: &str) -> bool;

    /// Non-streaming chat completion.
    fn chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>>;

    /// Streaming chat completion returning an SSE-compatible stream of chunks.
    fn stream_chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>,
                        ProviderError,
                    >,
                > + Send
                + '_,
        >,
    >;

    /// Lightweight health check (e.g. can we reach the API, do we have tokens?).
    fn health_check(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;

    /// Pricing data for each model this provider supports.
    fn pricing(&self) -> Vec<ModelPricing>;
}
