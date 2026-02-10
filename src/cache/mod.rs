pub mod embedder;
pub mod key;
pub mod store;
pub mod types;

use crate::config::{CacheConfig, CacheMode};
use crate::providers::types::{ChatRequest, ChatResponse};

use self::store::CacheStore;
use self::types::{CacheEntry, CacheError, CacheLookupResult, CacheStats, CacheStatsSnapshot};

// ---------------------------------------------------------------------------
// SemanticCache -- public facade
// ---------------------------------------------------------------------------

/// Opt-in semantic cache for non-streaming chat completions.
///
/// Two-tier lookup:
/// 1. Exact match (SHA-256 hash of normalized request fields)
/// 2. Semantic match (embedding KNN via SurrealDB HNSW)
pub struct SemanticCache {
    store: CacheStore,
    config: CacheConfig,
    stats: CacheStats,
}

impl SemanticCache {
    /// Initialize the cache with the given configuration.
    ///
    /// Creates an in-memory SurrealDB datastore and initializes the schema.
    pub async fn new(config: &CacheConfig) -> Result<Self, CacheError> {
        let store = CacheStore::new(
            config.embedding_dimension,
            config.hnsw_m,
            config.hnsw_ef_construction,
        )
        .await?;

        Ok(Self {
            store,
            config: config.clone(),
            stats: CacheStats::new(),
        })
    }

    /// Check whether this request should be checked against the cache.
    pub fn should_check(&self, request: &ChatRequest) -> bool {
        !key::should_skip(request, &self.config)
    }

    /// Look up a cached response for the given request.
    ///
    /// Tries exact match first, then semantic match (if mode is "semantic" or "both").
    pub async fn lookup(&self, request: &ChatRequest) -> Result<CacheLookupResult, CacheError> {
        let hash = key::exact_hash(request);

        // Tier 1: Exact match (always checked unless mode is semantic-only)
        if self.config.mode != CacheMode::Semantic {
            if let Some(entry) = self.store.lookup_exact(&hash, self.config.ttl_secs).await? {
                self.store.record_hit(&hash).await.ok();
                self.stats.record_exact_hit();
                return Ok(CacheLookupResult::ExactHit(entry));
            }
        }

        // Tier 2: Semantic match (requires embedder and mode includes semantic)
        if self.config.mode != CacheMode::Exact {
            if let Some(ref embedding_url) = self.config.embedding_url {
                let sem_text = key::semantic_text(request);
                if !sem_text.is_empty() {
                    match self.embed(&sem_text, embedding_url).await {
                        Ok(embedding) => {
                            if let Some((entry, similarity)) = self
                                .store
                                .lookup_semantic(
                                    &embedding,
                                    &request.model,
                                    10,
                                    self.config.similarity_threshold,
                                    self.config.ttl_secs,
                                )
                                .await?
                            {
                                self.store.record_hit(&entry.exact_hash).await.ok();
                                self.stats.record_semantic_hit();
                                return Ok(CacheLookupResult::SemanticHit { entry, similarity });
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Embedding lookup failed, falling back to miss");
                        }
                    }
                }
            }
        }

        self.stats.record_miss();
        Ok(CacheLookupResult::Miss)
    }

    /// Store a request→response pair in the cache.
    ///
    /// If the mode includes semantic, embeds the request text in the background.
    pub async fn store(
        &self,
        request: &ChatRequest,
        response: &ChatResponse,
    ) -> Result<(), CacheError> {
        // Only cache responses with finish_reason "stop" (no errors, no tool calls).
        let should_cache = response.choices.iter().any(|c| {
            c.finish_reason
                .as_deref()
                .is_some_and(|r| r == "stop")
        });
        if !should_cache {
            return Ok(());
        }

        let hash = key::exact_hash(request);
        let sem_text = key::semantic_text(request);
        let request_json = serde_json::to_string(request)
            .map_err(|e| CacheError::Serialization(e.to_string()))?;
        let response_json = serde_json::to_string(response)
            .map_err(|e| CacheError::Serialization(e.to_string()))?;

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
            exact_hash: hash,
            model: request.model.clone(),
            semantic_text: sem_text,
            embedding,
            request_json,
            response_json,
            created_at: String::new(), // Set by SurrealDB default
            hit_count: 0,
            last_hit: None,
        };

        self.store.insert(&entry).await?;

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
        embedder::embed(url, model, text, api_key).await
    }
}
