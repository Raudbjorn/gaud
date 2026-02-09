//! Provider Router
//!
//! Accepts a [`ChatRequest`] and routes it to the correct [`LlmProvider`]
//! based on the `model` field. Supports multiple routing strategies, circuit
//! breaker health tracking, and automatic fallback on failure.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use futures::Stream;
use rand::Rng;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::providers::health::{CircuitBreaker, CircuitState};
use crate::providers::types::{ChatChunk, ChatRequest, ChatResponse, ModelPricing};
use crate::providers::{LlmProvider, ProviderError};

// ---------------------------------------------------------------------------
// Routing Strategy
// ---------------------------------------------------------------------------

/// Strategy used to choose among healthy providers that support the requested
/// model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RoutingStrategy {
    /// Use providers in the order they were registered.
    #[default]
    Priority,
    /// Cycle through providers in round-robin order.
    RoundRobin,
    /// Pick the provider with the fewest total requests (approximation of
    /// least-used).
    LeastUsed,
    /// Pick a random healthy provider.
    Random,
}

// ---------------------------------------------------------------------------
// Per-provider stats
// ---------------------------------------------------------------------------

/// Simple request counters kept per provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderStats {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub total_latency_ms: u64,
}

impl ProviderStats {
    pub fn avg_latency_ms(&self) -> u64 {
        if self.successful_requests == 0 {
            return 0;
        }
        self.total_latency_ms / self.successful_requests
    }
}

// ---------------------------------------------------------------------------
// Registered provider entry
// ---------------------------------------------------------------------------

struct RegisteredProvider {
    provider: Arc<dyn LlmProvider>,
    circuit: CircuitBreaker,
    stats: ProviderStats,
}

// ---------------------------------------------------------------------------
// ProviderRouter
// ---------------------------------------------------------------------------

/// Routes incoming chat requests to the appropriate LLM provider based on the
/// requested model name.
pub struct ProviderRouter {
    /// Provider id -> registration entry.
    providers: HashMap<String, RegisteredProvider>,
    /// Insertion-order for priority routing.
    order: Vec<String>,
    /// Active routing strategy.
    strategy: RoutingStrategy,
    /// Round-robin counter.
    rr_index: usize,
}

impl ProviderRouter {
    /// Create a new router with the default (Priority) strategy.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            order: Vec::new(),
            strategy: RoutingStrategy::Priority,
            rr_index: 0,
        }
    }

    /// Create a new router with the given strategy.
    pub fn with_strategy(strategy: RoutingStrategy) -> Self {
        Self {
            strategy,
            ..Self::new()
        }
    }

    /// Register a provider. Providers are tried in registration order when
    /// using [`RoutingStrategy::Priority`].
    pub fn register(&mut self, provider: Arc<dyn LlmProvider>) {
        let id = provider.id().to_string();
        if self.providers.contains_key(&id) {
            warn!(provider = %id, "Provider already registered, replacing");
            self.order.retain(|o| o != &id);
        }
        self.order.push(id.clone());
        self.providers.insert(
            id,
            RegisteredProvider {
                provider,
                circuit: CircuitBreaker::new(),
                stats: ProviderStats::default(),
            },
        );
    }

    /// Change the routing strategy at runtime.
    pub fn set_strategy(&mut self, strategy: RoutingStrategy) {
        self.strategy = strategy;
    }

    /// Current routing strategy.
    pub fn strategy(&self) -> RoutingStrategy {
        self.strategy
    }

    // -- queries -------------------------------------------------------------

    /// List all available models across every registered provider.
    pub fn available_models(&self) -> Vec<(String, String)> {
        let mut models = Vec::new();
        for entry in self.providers.values() {
            let provider_id = entry.provider.id().to_string();
            for m in entry.provider.models() {
                models.push((m, provider_id.clone()));
            }
        }
        models
    }

    /// Get pricing data for every registered model.
    pub fn all_pricing(&self) -> Vec<ModelPricing> {
        self.providers
            .values()
            .flat_map(|e| e.provider.pricing())
            .collect()
    }

    /// Get circuit breaker state for a provider.
    pub fn circuit_state(&self, provider_id: &str) -> Option<CircuitState> {
        self.providers.get(provider_id).map(|e| e.circuit.state())
    }

    /// Get stats for a provider.
    pub fn stats(&self, provider_id: &str) -> Option<&ProviderStats> {
        self.providers.get(provider_id).map(|e| &e.stats)
    }

    /// Reset a provider's circuit breaker.
    pub fn reset_circuit(&mut self, provider_id: &str) {
        if let Some(e) = self.providers.get_mut(provider_id) {
            e.circuit.reset();
        }
    }

    /// Registered provider IDs in priority order.
    pub fn provider_ids(&self) -> &[String] {
        &self.order
    }

    // -- model -> provider resolution ----------------------------------------

    /// Determine which provider should handle the given model string.
    ///
    /// Mapping rules:
    ///   litellm:*          -> "litellm"
    ///   kiro:*             -> "kiro"
    ///   claude-*           -> "claude"
    ///   gemini-*           -> "gemini"
    ///   gpt-* | o1* | o3* -> "copilot"
    fn resolve_provider_id(model: &str) -> Option<&'static str> {
        if model.starts_with("litellm:") {
            return Some("litellm");
        }
        if model.starts_with("kiro:") {
            return Some("kiro");
        }
        if model.starts_with("claude-") || model.starts_with("claude_") {
            return Some("claude");
        }
        if model.starts_with("gemini-") || model.starts_with("gemini_") {
            return Some("gemini");
        }
        if model.starts_with("gpt-")
            || model.starts_with("o1")
            || model.starts_with("o3")
        {
            return Some("copilot");
        }
        None
    }

    /// Find all registered providers that can handle `model`, ordered by the
    /// active routing strategy.
    fn candidates_for_model(&mut self, model: &str) -> Vec<String> {
        // 1. Collect IDs of providers that support this model AND whose circuit
        //    breaker allows execution.
        let mut candidates: Vec<String> = Vec::new();

        // First try the prefix-mapped provider.
        if let Some(primary_id) = Self::resolve_provider_id(model) {
            if let Some(entry) = self.providers.get_mut(primary_id) {
                if entry.provider.supports_model(model) && entry.circuit.can_execute() {
                    candidates.push(primary_id.to_string());
                }
            }
        }

        // Then add any other provider that supports the model (fallback).
        for id in &self.order {
            if candidates.contains(id) {
                continue;
            }
            if let Some(entry) = self.providers.get_mut(id) {
                if entry.provider.supports_model(model) && entry.circuit.can_execute() {
                    candidates.push(id.clone());
                }
            }
        }

        // 2. Reorder according to strategy.
        match self.strategy {
            RoutingStrategy::Priority => { /* already in registration order */ }
            RoutingStrategy::RoundRobin => {
                if !candidates.is_empty() {
                    let start = self.rr_index % candidates.len();
                    candidates.rotate_left(start);
                    self.rr_index = self.rr_index.wrapping_add(1);
                }
            }
            RoutingStrategy::LeastUsed => {
                candidates.sort_by_key(|id| {
                    self.providers
                        .get(id)
                        .map(|e| e.stats.total_requests)
                        .unwrap_or(u64::MAX)
                });
            }
            RoutingStrategy::Random => {
                if candidates.len() > 1 {
                    // Fisher-Yates shuffle.
                    let mut rng = rand::rng();
                    for i in (1..candidates.len()).rev() {
                        let j = rng.random_range(0..=i);
                        candidates.swap(i, j);
                    }
                }
            }
        }

        candidates
    }

    // -- chat ----------------------------------------------------------------

    /// Route a non-streaming chat request. Tries each candidate provider in
    /// order and falls back on failure.
    pub async fn chat(&mut self, request: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let candidates = self.candidates_for_model(&request.model);
        if candidates.is_empty() {
            return Err(ProviderError::NoProvider(request.model.clone()));
        }

        let mut last_err: Option<ProviderError> = None;

        for id in &candidates {
            let provider = {
                let entry = self.providers.get(id).unwrap();
                Arc::clone(&entry.provider)
            };

            debug!(provider = %id, model = %request.model, "Attempting chat");
            let start = Instant::now();

            match provider.chat(request).await {
                Ok(response) => {
                    let latency_ms = start.elapsed().as_millis() as u64;
                    if let Some(entry) = self.providers.get_mut(id) {
                        entry.circuit.record_success();
                        entry.stats.successful_requests += 1;
                        entry.stats.total_requests += 1;
                        entry.stats.total_latency_ms += latency_ms;
                    }
                    info!(
                        provider = %id,
                        model = %request.model,
                        latency_ms,
                        "Chat succeeded"
                    );
                    return Ok(response);
                }
                Err(e) => {
                    warn!(provider = %id, error = %e, "Chat failed, trying next provider");
                    if let Some(entry) = self.providers.get_mut(id) {
                        entry.circuit.record_failure();
                        entry.stats.failed_requests += 1;
                        entry.stats.total_requests += 1;
                    }
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or(ProviderError::AllFailed))
    }

    /// Route a streaming chat request. Does NOT fall back -- returns an error
    /// from the first matching provider if it fails (because we cannot
    /// seamlessly splice streams mid-response).
    pub async fn stream_chat(
        &mut self,
        request: &ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>, ProviderError>
    {
        let candidates = self.candidates_for_model(&request.model);
        if candidates.is_empty() {
            return Err(ProviderError::NoProvider(request.model.clone()));
        }

        let mut last_err: Option<ProviderError> = None;

        for id in &candidates {
            let provider = {
                let entry = self.providers.get(id).unwrap();
                Arc::clone(&entry.provider)
            };

            debug!(provider = %id, model = %request.model, "Attempting stream_chat");

            match provider.stream_chat(request).await {
                Ok(stream) => {
                    if let Some(entry) = self.providers.get_mut(id) {
                        entry.stats.total_requests += 1;
                        // We record success optimistically for stream initiation;
                        // the actual success/failure of data delivery is handled
                        // by the caller consuming the stream.
                        entry.circuit.record_success();
                        entry.stats.successful_requests += 1;
                    }
                    info!(provider = %id, model = %request.model, "Stream started");
                    return Ok(stream);
                }
                Err(e) => {
                    warn!(provider = %id, error = %e, "Stream init failed, trying next");
                    if let Some(entry) = self.providers.get_mut(id) {
                        entry.circuit.record_failure();
                        entry.stats.failed_requests += 1;
                        entry.stats.total_requests += 1;
                    }
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or(ProviderError::AllFailed))
    }

    /// Run health checks against all registered providers.
    pub async fn health_check_all(&mut self) -> HashMap<String, bool> {
        let mut results = HashMap::new();
        let entries: Vec<(String, Arc<dyn LlmProvider>)> = self
            .providers
            .iter()
            .map(|(id, e)| (id.clone(), Arc::clone(&e.provider)))
            .collect();

        for (id, provider) in entries {
            let healthy = provider.health_check().await;
            if let Some(entry) = self.providers.get_mut(&id) {
                if healthy {
                    entry.circuit.record_success();
                } else {
                    entry.circuit.record_failure();
                }
            }
            results.insert(id, healthy);
        }
        results
    }
}

impl Default for ProviderRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::types::*;

    // A tiny stub provider for testing the router.
    struct StubProvider {
        id: &'static str,
        models: Vec<String>,
        should_fail: bool,
    }

    impl StubProvider {
        fn new(id: &'static str, models: &[&str]) -> Self {
            Self {
                id,
                models: models.iter().map(|s| s.to_string()).collect(),
                should_fail: false,
            }
        }

        fn failing(id: &'static str, models: &[&str]) -> Self {
            Self {
                id,
                models: models.iter().map(|s| s.to_string()).collect(),
                should_fail: true,
            }
        }
    }

    impl LlmProvider for StubProvider {
        fn id(&self) -> &str {
            self.id
        }

        fn name(&self) -> &str {
            self.id
        }

        fn models(&self) -> Vec<String> {
            self.models.clone()
        }

        fn supports_model(&self, model: &str) -> bool {
            self.models.iter().any(|m| m == model)
        }

        fn chat(
            &self,
            request: &ChatRequest,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
            let model = request.model.clone();
            let should_fail = self.should_fail;
            Box::pin(async move {
                if should_fail {
                    return Err(ProviderError::Other("stub failure".into()));
                }
                Ok(ChatResponse {
                    id: "resp-1".into(),
                    object: "chat.completion".into(),
                    created: 0,
                    model,
                    choices: vec![Choice {
                        index: 0,
                        message: ResponseMessage {
                            role: "assistant".into(),
                            content: Some("Hello from stub".into()),
                            tool_calls: None,
                        },
                        finish_reason: Some("stop".into()),
                    }],
                    usage: Usage::default(),
                })
            })
        }

        fn stream_chat(
            &self,
            _request: &ChatRequest,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>, ProviderError>> + Send + '_>> {
            let should_fail = self.should_fail;
            Box::pin(async move {
                if should_fail {
                    return Err(ProviderError::Other("stub stream failure".into()));
                }
                let stream = futures::stream::once(async {
                    Ok(ChatChunk {
                        id: "chunk-1".into(),
                        object: "chat.completion.chunk".into(),
                        created: 0,
                        model: "test".into(),
                        choices: vec![],
                        usage: None,
                    })
                });
                Ok(Box::pin(stream) as Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>)
            })
        }

        fn health_check(&self) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
            let should_fail = self.should_fail;
            Box::pin(async move { !should_fail })
        }

        fn pricing(&self) -> Vec<ModelPricing> {
            vec![]
        }
    }

    fn make_request(model: &str) -> ChatRequest {
        ChatRequest {
            model: model.into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hi".into())),
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
        }
    }

    #[test]
    fn test_resolve_provider_claude() {
        assert_eq!(
            ProviderRouter::resolve_provider_id("claude-sonnet-4-20250514"),
            Some("claude")
        );
    }

    #[test]
    fn test_resolve_provider_gemini() {
        assert_eq!(
            ProviderRouter::resolve_provider_id("gemini-2.5-flash"),
            Some("gemini")
        );
    }

    #[test]
    fn test_resolve_provider_copilot_gpt() {
        assert_eq!(
            ProviderRouter::resolve_provider_id("gpt-4o"),
            Some("copilot")
        );
    }

    #[test]
    fn test_resolve_provider_copilot_o1() {
        assert_eq!(
            ProviderRouter::resolve_provider_id("o1"),
            Some("copilot")
        );
    }

    #[test]
    fn test_resolve_provider_copilot_o3() {
        assert_eq!(
            ProviderRouter::resolve_provider_id("o3-mini"),
            Some("copilot")
        );
    }

    #[test]
    fn test_resolve_provider_kiro() {
        assert_eq!(
            ProviderRouter::resolve_provider_id("kiro:claude-sonnet-4"),
            Some("kiro")
        );
        assert_eq!(
            ProviderRouter::resolve_provider_id("kiro:auto"),
            Some("kiro")
        );
    }

    #[test]
    fn test_resolve_provider_litellm() {
        assert_eq!(
            ProviderRouter::resolve_provider_id("litellm:gpt-4o"),
            Some("litellm")
        );
        assert_eq!(
            ProviderRouter::resolve_provider_id("litellm:anthropic/claude-3"),
            Some("litellm")
        );
    }

    #[test]
    fn test_resolve_provider_unknown() {
        assert_eq!(
            ProviderRouter::resolve_provider_id("llama-3.3-70b"),
            None
        );
    }

    #[tokio::test]
    async fn test_chat_routes_correctly() {
        let mut router = ProviderRouter::new();
        router.register(Arc::new(StubProvider::new(
            "claude",
            &["claude-sonnet-4-20250514"],
        )));
        router.register(Arc::new(StubProvider::new("copilot", &["gpt-4o"])));

        let resp = router.chat(&make_request("claude-sonnet-4-20250514")).await;
        assert!(resp.is_ok());
        let resp = resp.unwrap();
        assert_eq!(resp.model, "claude-sonnet-4-20250514");
    }

    #[tokio::test]
    async fn test_no_provider_returns_error() {
        let mut router = ProviderRouter::new();
        router.register(Arc::new(StubProvider::new("claude", &["claude-sonnet-4-20250514"])));

        let result = router.chat(&make_request("nonexistent-model")).await;
        assert!(matches!(result, Err(ProviderError::NoProvider(_))));
    }

    #[tokio::test]
    async fn test_fallback_on_failure() {
        let mut router = ProviderRouter::new();
        // Register a failing claude provider and a working one with a
        // different ID that also supports the same model.
        router.register(Arc::new(StubProvider::failing(
            "claude",
            &["claude-sonnet-4-20250514"],
        )));
        // A hypothetical fallback provider.
        router.register(Arc::new(StubProvider::new(
            "claude-backup",
            &["claude-sonnet-4-20250514"],
        )));

        let resp = router.chat(&make_request("claude-sonnet-4-20250514")).await;
        assert!(resp.is_ok());
    }

    #[tokio::test]
    async fn test_all_fail_returns_error() {
        let mut router = ProviderRouter::new();
        router.register(Arc::new(StubProvider::failing(
            "claude",
            &["claude-sonnet-4-20250514"],
        )));

        let result = router.chat(&make_request("claude-sonnet-4-20250514")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_available_models() {
        let mut router = ProviderRouter::new();
        router.register(Arc::new(StubProvider::new(
            "claude",
            &["claude-sonnet-4-20250514", "claude-opus-4-20250514"],
        )));
        router.register(Arc::new(StubProvider::new("copilot", &["gpt-4o", "o3-mini"])));

        let models = router.available_models();
        assert_eq!(models.len(), 4);
    }

    #[tokio::test]
    async fn test_circuit_breaker_trips() {
        let mut router = ProviderRouter::new();
        router.register(Arc::new(StubProvider::failing(
            "claude",
            &["claude-sonnet-4-20250514"],
        )));

        // Fail 3 times to trip the circuit.
        for _ in 0..3 {
            let _ = router.chat(&make_request("claude-sonnet-4-20250514")).await;
        }

        assert_eq!(
            router.circuit_state("claude"),
            Some(CircuitState::Open)
        );

        // Now the circuit is open -- provider won't even be tried.
        let result = router.chat(&make_request("claude-sonnet-4-20250514")).await;
        assert!(matches!(result, Err(ProviderError::NoProvider(_))));
    }

    #[tokio::test]
    async fn test_health_check_all() {
        let mut router = ProviderRouter::new();
        router.register(Arc::new(StubProvider::new(
            "claude",
            &["claude-sonnet-4-20250514"],
        )));
        router.register(Arc::new(StubProvider::failing("copilot", &["gpt-4o"])));

        let results = router.health_check_all().await;
        assert_eq!(results.get("claude"), Some(&true));
        assert_eq!(results.get("copilot"), Some(&false));
    }

    #[test]
    fn test_stats_default() {
        let stats = ProviderStats::default();
        assert_eq!(stats.total_requests, 0);
        assert_eq!(stats.avg_latency_ms(), 0);
    }

    #[tokio::test]
    async fn test_stats_update_on_success() {
        let mut router = ProviderRouter::new();
        router.register(Arc::new(StubProvider::new(
            "claude",
            &["claude-sonnet-4-20250514"],
        )));

        let _ = router.chat(&make_request("claude-sonnet-4-20250514")).await;

        let stats = router.stats("claude").unwrap();
        assert_eq!(stats.total_requests, 1);
        assert_eq!(stats.successful_requests, 1);
        assert_eq!(stats.failed_requests, 0);
    }

    #[tokio::test]
    async fn test_stats_update_on_failure() {
        let mut router = ProviderRouter::new();
        router.register(Arc::new(StubProvider::failing(
            "claude",
            &["claude-sonnet-4-20250514"],
        )));

        let _ = router.chat(&make_request("claude-sonnet-4-20250514")).await;

        let stats = router.stats("claude").unwrap();
        assert_eq!(stats.total_requests, 1);
        assert_eq!(stats.successful_requests, 0);
        assert_eq!(stats.failed_requests, 1);
    }

    #[test]
    fn test_set_strategy() {
        let mut router = ProviderRouter::new();
        assert_eq!(router.strategy(), RoutingStrategy::Priority);

        router.set_strategy(RoutingStrategy::RoundRobin);
        assert_eq!(router.strategy(), RoutingStrategy::RoundRobin);
    }

    #[tokio::test]
    async fn test_reset_circuit() {
        let mut router = ProviderRouter::new();
        router.register(Arc::new(StubProvider::failing(
            "claude",
            &["claude-sonnet-4-20250514"],
        )));

        for _ in 0..3 {
            let _ = router.chat(&make_request("claude-sonnet-4-20250514")).await;
        }
        assert_eq!(router.circuit_state("claude"), Some(CircuitState::Open));

        router.reset_circuit("claude");
        assert_eq!(router.circuit_state("claude"), Some(CircuitState::Closed));
    }

    #[test]
    fn test_provider_ids_order() {
        let mut router = ProviderRouter::new();
        router.register(Arc::new(StubProvider::new("claude", &[])));
        router.register(Arc::new(StubProvider::new("gemini", &[])));
        router.register(Arc::new(StubProvider::new("copilot", &[])));

        assert_eq!(router.provider_ids(), &["claude", "gemini", "copilot"]);
    }
}
