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
use crate::providers::types::{ChatRequest, ChatResponse};

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

    /// Check whether this request should be checked against the cache.
    pub fn should_check(&self, request: &ChatRequest) -> bool {
        !key::should_skip(request, &self.config)
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
}
