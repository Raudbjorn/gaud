// ---------------------------------------------------------------------------
// Feature Guard
// ---------------------------------------------------------------------------

#[cfg(all(feature = "cache-persistent", feature = "cache-ephemeral"))]
compile_error!("Cannot enable both 'cache-persistent' and 'cache-ephemeral' features at the same time.");

pub mod embedder;

pub mod key;
pub mod store;
pub mod types;

use crate::config::{CacheConfig, CacheMode};
use crate::providers::types::{
    ChatRequest, ChatResponse, ChatChunk,
    Choice, ResponseMessage, Usage
};

use std::future::Future;
use std::pin::Pin;

use self::store::CacheStore;
use self::types::{
    CacheEntry, CacheError, CacheHitKind, CacheLookupResult, CacheMetadata,
    CacheStats, CacheStatsSnapshot,
};

// ---------------------------------------------------------------------------
// SemanticCacheService -- public facade
// ---------------------------------------------------------------------------

/// High-level semantic cache service that bridges between chat requests and the low-level store.
pub struct SemanticCacheService {
    store: CacheStore,
    config: CacheConfig,
    stats: CacheStats,
}

impl SemanticCacheService {
    /// Initialize the cache service with the given configuration.
    pub async fn new(config: &CacheConfig) -> Result<Self, CacheError> {
        #[cfg(feature = "cache-persistent")]
        let store = CacheStore::persistent(
            config.path.to_str().unwrap_or("gaud.cache"),
            config.embedding_dimension,
        )
        .await?;

        #[cfg(all(not(feature = "cache-persistent"), feature = "cache-ephemeral"))]
        let store = CacheStore::ephemeral(config.embedding_dimension).await?;

        #[cfg(all(not(feature = "cache-persistent"), not(feature = "cache-ephemeral")))]
        return Err(CacheError::InitFailed("No cache storage backend enabled (persistent or ephemeral)".into()));

        Ok(Self {
            store,
            config: config.clone(),
            stats: CacheStats::new(),
        })
    }

    /// Helper for tests to inject a pre-populated store.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn new_with_store(store: CacheStore, config: CacheConfig) -> Self {
        Self {
            store,
            config,
            stats: CacheStats::new(),
        }
    }

    /// Check whether this request should be checked against the cache.
    pub fn should_check(&self, request: &ChatRequest) -> bool {
        !key::should_skip(request, &self.config)
    }

    /// Check whether this *streaming* request should be checked against the
    /// streaming replay cache.
    pub fn should_check_stream(&self, request: &ChatRequest) -> bool {
        self.config.enabled && self.config.stream_cache_enabled && !key::should_skip_stream(request, &self.config)
    }

    /// Look up a cached response for the given request.
    pub async fn lookup(&self, request: &ChatRequest) -> Result<CacheLookupResult, CacheError> {
        let exact_hash = key::exact_hash(request);

        let metadata = CacheMetadata {
            model: request.model.clone(),
            system_prompt_hash: key::system_prompt_hash(request),
            tool_definitions_hash: key::tool_definitions_hash(request),
            temperature: request.temperature,
            confidence: None, // Could be calculated from model response in the future
        };

        // Get embedding if semantic mode is enabled
        let embedding = if self.config.mode != CacheMode::Exact {
            if let Some(ref url) = self.config.embedding_url {
                let sem_text = key::semantic_text(request);
                if !sem_text.is_empty() {
                    match self.embed(&sem_text, url).await {
                        Ok(emb) => Some(emb),
                        Err(e) => {
                            tracing::warn!(error = %e, "Embedding lookup failed, falling back to exact-only");
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let result = self.store.lookup(
            &exact_hash,
            embedding.as_deref(),
            &metadata,
            self.config.similarity_threshold,
            self.config.ttl_secs,
        ).await?;

        match &result {
            CacheLookupResult::Hit(entry, info) => {
                self.store.record_hit(&entry.exact_hash).await.ok();
                match info.kind {
                    CacheHitKind::Exact => self.stats.record_exact_hit(),
                    CacheHitKind::Semantic => self.stats.record_semantic_hit(),
                }
            }
            CacheLookupResult::Miss => {
                self.stats.record_miss();
            }
        }

        Ok(result)
    }

    /// Store a request→response pair in the cache.
    pub async fn store(
        &self,
        request: &ChatRequest,
        response: &ChatResponse,
    ) -> Result<(), CacheError> {
        // Only cache responses with finish_reason "stop"
        let should_cache = response.choices.iter().any(|c| {
            c.finish_reason
                .as_deref()
                .is_some_and(|r| r == "stop")
        });
        if !should_cache {
            return Ok(());
        }

        let exact_hash = key::exact_hash(request);
        let sem_text = key::semantic_text(request);
        let request_json = serde_json::to_string(request)?;
        let response_json = serde_json::to_string(response)?;

        let metadata = CacheMetadata {
            model: request.model.clone(),
            system_prompt_hash: key::system_prompt_hash(request),
            tool_definitions_hash: key::tool_definitions_hash(request),
            temperature: request.temperature,
            confidence: None,
        };

        // Get embedding if semantic mode is enabled
        let embedding = if self.config.mode != CacheMode::Exact {
            if let Some(ref url) = self.config.embedding_url {
                match self.embed(&sem_text, url).await {
                    Ok(emb) => Some(emb),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to embed for cache store, storing without embedding");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let entry = CacheEntry {
            exact_hash,
            model: request.model.clone(),
            system_prompt_hash: metadata.system_prompt_hash.clone(),
            tool_definitions_hash: metadata.tool_definitions_hash.clone(),
            semantic_text: sem_text,
            embedding,
            request_json,
            response_json,
            created_at: srrldb::types::Datetime::now(), // Set by SurrealDB but placeholder here
            hit_count: 0,
            last_hit: None,
            hash_version: "v1".to_string(),
            stream_events: None,
            stream_format: None,
        };

        self.store.insert(&entry, &metadata).await?;

        // Enforce max_entries limit
        if self.config.max_entries > 0 {
            self.store.evict_lru(self.config.max_entries).await.ok();
        }

        Ok(())
    }

    /// Evict entries older than TTL.
    pub async fn evict_expired(&self, ttl_secs: u64) -> Result<u64, CacheError> {
        self.store.evict_expired(ttl_secs).await
    }

    /// Flush all cache entries.
    pub async fn flush_all(&self) -> Result<(), CacheError> {
        self.store.flush_all().await
    }

    /// Flush entries for a specific model.
    pub async fn flush_model(&self, model: &str) -> Result<(), CacheError> {
        self.store.flush_model(model).await
    }

    /// Get cache statistics snapshot.
    pub fn stats(&self) -> CacheStatsSnapshot {
        self.stats.snapshot()
    }

    /// Get total entry count.
    pub async fn count(&self) -> Result<u64, CacheError> {
        self.store.count().await
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Call the embedding API to get a vector for the given text.
    async fn embed(&self, text: &str, url: &str) -> Result<Vec<f32>, CacheError> {
        let model = self
            .config
            .embedding_model
            .as_deref()
            .unwrap_or("text-embedding-3-small");
        let api_key = self.config.embedding_api_key.as_deref();

        let mut embedding = embedder::embed(url, model, text, api_key, self.config.embedding_allow_local).await?;

        // Ensure normalization for cosine distance
        self.normalize(&mut embedding);

        Ok(embedding)
    }

    fn normalize(&self, v: &mut [f32]) {
        let mag = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if mag > 1e-6 {
            v.iter_mut().for_each(|x| *x /= mag);
        }
    }

    fn build_metadata(&self, request: &ChatRequest) -> CacheMetadata {
        CacheMetadata {
            model: request.model.clone(),
            system_prompt_hash: key::system_prompt_hash(request),
            tool_definitions_hash: key::tool_definitions_hash(request),
            temperature: request.temperature,
            confidence: None,
        }
    }

    async fn resolve_embedding(&self, request: &ChatRequest) -> Option<Vec<f32>> {
        if self.config.mode == CacheMode::Exact {
            return None;
        }
        let url = self.config.embedding_url.as_ref()?;
        let sem_text = key::semantic_text(request);
        if sem_text.is_empty() {
            return None;
        }
        match self.embed(&sem_text, url).await {
            Ok(emb) => Some(emb),
            Err(e) => {
                tracing::warn!(error = %e, "Embedding lookup failed, falling back to exact-only");
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Streaming cache methods
// ---------------------------------------------------------------------------

impl SemanticCacheService {
    /// Look up cached stream events for the given request.
    ///
    /// Returns `CacheLookupResult::Hit` only if the matched entry contains
    /// `stream_events`. Otherwise returns `Miss`.
    pub async fn lookup_stream(
        &self,
        request: &ChatRequest,
    ) -> Result<CacheLookupResult, CacheError> {
        let exact_hash = key::exact_hash(request);
        let metadata = self.build_metadata(request);
        let embedding = self.resolve_embedding(request).await;

        let result = self.store.lookup(
            &exact_hash,
            embedding.as_deref(),
            &metadata,
            self.config.similarity_threshold,
            self.config.ttl_secs,
        ).await?;

        match &result {
            CacheLookupResult::Hit(entry, info) if entry.stream_events.is_some() => {
                if let Err(e) = self.store.record_hit(&entry.exact_hash).await {
                    tracing::warn!("Failed to record cache hit: {}", e);
                }
                match info.kind {
                    CacheHitKind::Exact => self.stats.record_stream_exact_hit(),
                    CacheHitKind::Semantic => self.stats.record_stream_semantic_hit(),
                }
                Ok(result)
            }
            CacheLookupResult::Hit(_, _) => {
                // Entry exists but has no stream events — treat as stream miss.
                // Do NOT record as a general cache miss to avoid skewing non-stream stats.
                self.stats.record_stream_miss();
                Ok(CacheLookupResult::Miss)
            }
            CacheLookupResult::Miss => {
                self.stats.record_stream_miss();
                Ok(CacheLookupResult::Miss)
            }
        }
    }

    /// Store stream events for a completed streaming response.
    ///
    /// Called in a background task after the stream finishes successfully.
    pub async fn store_stream(
        &self,
        request: &ChatRequest,
        events: &[String],
    ) -> Result<(), CacheError> {
        let exact_hash = key::exact_hash(request);
        let sem_text = key::semantic_text(request);
        let request_json = serde_json::to_string(request)?;
        let metadata = self.build_metadata(request);
        let embedding = self.resolve_embedding(request).await;

        let entry = CacheEntry {
            exact_hash,
            model: request.model.clone(),
            system_prompt_hash: metadata.system_prompt_hash.clone(),
            tool_definitions_hash: metadata.tool_definitions_hash.clone(),
            semantic_text: sem_text,
            embedding,
            request_json,
            response_json: String::new(), // No assembled response for stream-only
            created_at: srrldb::types::Datetime::now(),
            hit_count: 0,
            last_hit: None,
            hash_version: "v1".to_string(),
            stream_events: Some(events.to_vec()),
            stream_format: Some("openai_sse_v1".to_string()),
        };

        // Try to reconstruct a full response for non-stream compatibility
        let mut entry = entry;
        if let Ok(full_resp) = Self::reconstruct_response(request, events) {
             if let Ok(json) = serde_json::to_string(&full_resp) {
                 entry.response_json = json;
             }
        }

        self.store.insert_stream(&entry, &metadata, events).await?;

        // Enforce max_entries limit
        if self.config.max_entries > 0 {
            self.store.evict_lru(self.config.max_entries).await.ok();
        }

        Ok(())
    }

    /// Stream cache config: max events per request.
    pub fn stream_cache_max_events(&self) -> usize {
        self.config.stream_cache_max_events
    }

    pub fn stream_cache_max_bytes(&self) -> usize {
        self.config.stream_cache_max_bytes
    }

    /// Reconstruct a full ChatResponse from a sequence of SSE chunks.
    fn reconstruct_response(
        _request: &ChatRequest,
        events: &[String],
    ) -> Result<ChatResponse, CacheError> {
        let mut full_content = String::new();
        let mut id = String::new();
        let mut model = String::new();
        let mut created = 0;
        let mut finish_reason = None;
        let mut final_usage = Usage::default();

        for event_str in events {
            // events are raw JSON strings (payloads)
            if let Ok(chunk) = serde_json::from_str::<ChatChunk>(event_str) {
                if id.is_empty() {
                    id = chunk.id.clone();
                    model = chunk.model.clone();
                    created = chunk.created;
                }
                if let Some(choices) = chunk.choices.first() {
                    if let Some(ref content) = choices.delta.content {
                        full_content.push_str(content);
                    }
                    if choices.finish_reason.is_some() {
                        finish_reason = choices.finish_reason.clone();
                    }
                }
                if let Some(usage) = chunk.usage {
                    final_usage = usage;
                }
            }
        }

        if id.is_empty() {
             return Err(CacheError::Serialization("Empty stream or invalid chunks".into()));
        }

        Ok(ChatResponse {
            id,
            object: "chat.completion".into(),
            created,
            model,
            choices: vec![Choice {
                index: 0,
                message: ResponseMessage {
                    role: "assistant".into(),
                    content: Some(full_content),
                    reasoning_content: None,
                    tool_calls: None, // TODO: support tool calls reconstruction
                },
                finish_reason,
            }],
            usage: final_usage,
        })
    }
}

// ---------------------------------------------------------------------------
// StreamCacheOps trait — testable interface
// ---------------------------------------------------------------------------

/// Trait for streaming cache operations.
///
/// Implemented by [`SemanticCacheService`] in production and by fakes in tests.
/// Using a trait here avoids coupling the stream handler to the concrete cache
/// implementation, making it possible to test without SurrealDB.
pub trait StreamCacheOps: Send + Sync {
    /// Whether this streaming request should be checked against the cache.
    fn check_stream(&self, request: &ChatRequest) -> bool;

    /// Look up cached stream events. Returns `Some((events, hit_kind))` on hit.
    fn get_cached_events<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(Vec<String>, &'static str)>, CacheError>> + Send + 'a>>;

    /// Store stream events for a completed streaming response.
    fn put_stream_events<'a>(
        &'a self,
        request: &'a ChatRequest,
        events: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>>;

    /// Max events to buffer per streaming request.
    fn max_stream_events(&self) -> usize;

    /// Max bytes to buffer per streaming request.
    fn max_stream_bytes(&self) -> usize;
}

impl StreamCacheOps for SemanticCacheService {
    fn check_stream(&self, request: &ChatRequest) -> bool {
        self.should_check_stream(request)
    }

    fn get_cached_events<'a>(
        &'a self,
        request: &'a ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(Vec<String>, &'static str)>, CacheError>> + Send + 'a>> {
        Box::pin(async move {
            match self.lookup_stream(request).await? {
                CacheLookupResult::Hit(entry, info) => {
                    let kind = match info.kind {
                        CacheHitKind::Exact => "exact",
                        CacheHitKind::Semantic => "semantic",
                    };
                    Ok(entry.stream_events.map(|events| (events, kind)))
                }
                CacheLookupResult::Miss => Ok(None),
            }
        })
    }

    fn put_stream_events<'a>(
        &'a self,
        request: &'a ChatRequest,
        events: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>> {
        Box::pin(async move {
            self.store_stream(request, &events).await
        })
    }

    fn max_stream_events(&self) -> usize {
        self.stream_cache_max_events()
    }

    fn max_stream_bytes(&self) -> usize {
        self.stream_cache_max_bytes()
    }
}

#[cfg(test)]
#[cfg(feature = "cache-ephemeral")]
mod tests {
    use crate::cache::store::CacheStore;
    use crate::providers::types::{ChatRequest, ChatMessage, MessageRole, MessageContent};

    #[allow(dead_code)]
    fn test_request() -> ChatRequest {
        ChatRequest {
            model: "test-model".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("hello".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            top_p: None,
            stream: true,
            stop: None,
            max_tokens: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        }
    }

    #[tokio::test]
    async fn test_service_stream_hit_no_events_returns_miss() {
        let store = Arc::new(CacheStore::ephemeral(3).await.expect("ephemeral init"));

        let request = test_request();
        let exact = crate::cache::key::exact_hash(&request);
        let sys = crate::cache::key::system_prompt_hash(&request);
        let tools = crate::cache::key::tool_definitions_hash(&request);
        let sem = crate::cache::key::semantic_text(&request);

        // Insert entry with NO stream_events
        let metadata = crate::cache::types::CacheMetadata {
            model: "test-model".into(),
            system_prompt_hash: sys.clone(),
            tool_definitions_hash: tools.clone(),
            temperature: None,
            confidence: None,
        };
        let entry = crate::cache::types::CacheEntry {
            exact_hash: exact,
            model: "test-model".into(),
            system_prompt_hash: sys,
            tool_definitions_hash: tools,
            semantic_text: sem,
            embedding: Some(vec![1.0, 0.0, 0.0]),
            request_json: "{}".into(),
            response_json: "{}".into(),
            created_at: srrldb::types::Datetime::now(),
            hit_count: 0,
            last_hit: None,
            hash_version: "v1".into(),
            stream_events: None, // KEY: This is what triggers the miss behavior
            stream_format: None,
        };
        store.insert(&entry, &metadata).await.expect("insert failed");

        let service = SemanticCacheService::new_with_store(store.as_ref().clone(), crate::config::CacheConfig::default());
        let ops: &dyn StreamCacheOps = &service;

        // Verify check_stream passes (assuming config defaults allow it)
        // Default CacheConfig has enabled=true? Let's check.
        // If not, we might need to configure it.
        // But assuming check_stream=true:

        // Act: Lookup
        let result = ops.get_cached_events(&request).await.expect("lookup failed");

        // Assert: Should be None even though we found an exact match, because stream_events is None
        assert!(result.is_none(), "Entry without stream_events must act as a cache miss");
    }
    #[tokio::test]
    async fn test_stream_stats_separation() {
        let store = Arc::new(CacheStore::ephemeral(4).await.expect("ephemeral init"));
        let service = SemanticCacheService::new_with_store(store.as_ref().clone(), crate::config::CacheConfig::default());
        let request = test_request();

        // 1. Stream lookup miss
        let _ = service.lookup_stream(&request).await.expect("lookup failed");
        let stats = service.stats();
        assert_eq!(stats.misses_stream, 1, "Should increment stream misses");
        assert_eq!(stats.misses, 0, "Should NOT increment global misses");

        // 2. Store stream events
        let events = vec!["data: event1\n\n".to_string(), "data: [DONE]\n\n".to_string()];
        service.store_stream(&request, &events).await.expect("store failed");

        // 3. Stream lookup hit
        let _ = service.lookup_stream(&request).await.expect("lookup failed");
        let stats = service.stats();
        assert_eq!(stats.hits_stream_exact, 1, "Should increment stream hits");
        assert_eq!(stats.hits_exact, 0, "Should NOT increment global hits");

        // 4. Non-stream lookup hit (should hit the same entry because we reconstruct response_json)
        // Wait, store_stream now reconstructs response_json!
        // So a normal lookup SHOULD hit.
        let _ = service.lookup(&request).await.expect("lookup failed");
        let stats = service.stats();
        assert_eq!(stats.hits_exact, 1, "Should increment global hits");
        assert_eq!(stats.hits_stream_exact, 1, "Stream hits should remain same");
    }
}
