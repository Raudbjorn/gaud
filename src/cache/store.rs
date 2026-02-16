use rand::Rng;
use srrldb::Database;

use crate::cache::types::{
    CacheEntry, CacheError, CacheHitInfo, CacheHitKind, CacheLookupResult, CacheMetadata,
};
use srrldb::types::SurrealValue;

// ---------------------------------------------------------------------------
// Error mapping helpers
// ---------------------------------------------------------------------------

/// Extension trait to reduce `.map_err(|e| CacheError::X(e.to_string()))` boilerplate.
trait MapCacheErr<T> {
    fn lookup_err(self) -> Result<T, CacheError>;
    fn insert_err(self) -> Result<T, CacheError>;
    fn schema_err(self) -> Result<T, CacheError>;
}

impl<T, E: std::fmt::Display> MapCacheErr<T> for Result<T, E> {
    fn lookup_err(self) -> Result<T, CacheError> {
        self.map_err(|e| CacheError::LookupFailed(e.to_string()))
    }
    fn insert_err(self) -> Result<T, CacheError> {
        self.map_err(|e| CacheError::InsertFailed(e.to_string()))
    }
    fn schema_err(self) -> Result<T, CacheError> {
        self.map_err(|e| CacheError::SchemaFailed(e.to_string()))
    }
}

/// A thin, vector-aware cache layer over an embedded SurrealDB instance.
///
/// `CacheStore` provides two-tier lookup (exact hash → ANN vector search)
/// for LLM prompt/response pairs. All storage, indexing, and transaction
/// Wrapper around the embedded semantic cache storage engine.
#[derive(Clone)]
pub struct CacheStore {
    db: Database,
    dimension: u16,
    hash_version: String,
}

impl CacheStore {
    /// Initialize a persistent semantic cache backed by RocksDB.
    ///
    /// Includes a warmup step that issues a synthetic ANN query to eager-load
    /// the HNSW index (only relevant for persistent storage where data survives
    /// restarts).
    #[cfg(feature = "cache-persistent")]
    pub async fn persistent(path: &str, dimension: u16) -> Result<Self, CacheError> {
        let mut db = Database::new_rocksdb(path)
            .await
            .map_err(|e| CacheError::InitFailed(e.to_string()))?;

        db.use_ns_db("gaud", "cache")
            .await
            .map_err(|e| CacheError::InitFailed(e.to_string()))?;

        let store = Self {
            db,
            dimension,
            hash_version: "v1".to_string(),
        };
        store.apply_schema().await?;
        store.warmup().await?;

        Ok(store)
    }

    /// Initialize an ephemeral in-memory cache. Suitable for testing.
    #[cfg(feature = "cache-ephemeral")]
    pub async fn ephemeral(dimension: u16) -> Result<Self, CacheError> {
        let mut db = Database::new_mem()
            .await
            .map_err(|e| CacheError::InitFailed(e.to_string()))?;

        db.use_ns_db("gaud", "cache")
            .await
            .map_err(|e| CacheError::InitFailed(e.to_string()))?;

        let store = Self {
            db,
            dimension,
            hash_version: "v1".to_string(),
        };
        store.apply_schema().await?;

        Ok(store)
    }

    /// Apply schema with versioning and compatibility checks.
    async fn apply_schema(&self) -> Result<(), CacheError> {
        // 1. Schema versioning
        self.db
            .query(
                "DEFINE TABLE IF NOT EXISTS schema_version SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS version ON schema_version TYPE int;",
            )
            .await
            .schema_err()?;

        let mut response = self
            .db
            .query("SELECT version FROM schema_version LIMIT 1")
            .await
            .schema_err()?;

        let version: Option<i64> = response.take(0usize).schema_err().ok().and_then(|v| {
            if let srrldb::types::Value::Array(arr) = v {
                arr.first().and_then(|row| {
                    if let srrldb::types::Value::Object(obj) = row {
                        obj.get("version").and_then(|v| match v {
                            srrldb::types::Value::Number(n) => n.to_int(),
                            _ => None,
                        })
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        });

        // Current schema version is 1
        const CURRENT_VERSION: i64 = 1;

        if let Some(v) = version {
            if v < CURRENT_VERSION {
                tracing::info!(current = v, target = CURRENT_VERSION, "Migrating cache schema");
                // Migration logic would go here if needed
            }
        } else {
            self.db
                .query("INSERT INTO schema_version { version: $v }")
                .bind(("v", CURRENT_VERSION))
                .await
                .schema_err()?;
        }

        // 2. Main cache table
        let schema = format!(
            r#"
            DEFINE TABLE IF NOT EXISTS cache SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS exact_hash            ON cache TYPE string;
            DEFINE FIELD IF NOT EXISTS model                 ON cache TYPE string;
            DEFINE FIELD IF NOT EXISTS system_prompt_hash    ON cache TYPE string;
            DEFINE FIELD IF NOT EXISTS tool_definitions_hash ON cache TYPE string;
            DEFINE FIELD IF NOT EXISTS semantic_text          ON cache TYPE string;
            DEFINE FIELD IF NOT EXISTS embedding              ON cache TYPE option<array<float>>;
            DEFINE FIELD IF NOT EXISTS request_json           ON cache TYPE string;
            DEFINE FIELD IF NOT EXISTS response_json          ON cache TYPE string;
            DEFINE FIELD IF NOT EXISTS created_at             ON cache TYPE datetime DEFAULT time::now();
            DEFINE FIELD IF NOT EXISTS hit_count              ON cache TYPE int DEFAULT 0;
            DEFINE FIELD IF NOT EXISTS last_hit               ON cache TYPE option<datetime>;
            DEFINE FIELD IF NOT EXISTS hash_version           ON cache TYPE string;
            DEFINE FIELD IF NOT EXISTS temperature            ON cache TYPE option<float>;
            DEFINE FIELD IF NOT EXISTS confidence             ON cache TYPE option<float>;

            DEFINE FIELD IF NOT EXISTS stream_events           ON cache TYPE option<array<string>>;
            DEFINE FIELD IF NOT EXISTS stream_format           ON cache TYPE option<string>;

            DEFINE INDEX IF NOT EXISTS idx_prompt_hash ON cache FIELDS exact_hash UNIQUE;
            DEFINE INDEX IF NOT EXISTS hnsw_embedding ON cache FIELDS embedding
                HNSW DIMENSION {dim} DIST COSINE;
            "#,
            dim = self.dimension
        );

        self.db.query(&schema).await.schema_err()?;

        // 3. Compatibility Guard: Verify embedding dimension
        // We can check if any existing row has a different dimension.
        let mut response = self
            .db
            .query("SELECT array::len(embedding) AS len FROM cache WHERE embedding IS NOT NONE LIMIT 1")
            .await
            .schema_err()?;

        if let Ok(v) = response.take::<srrldb::types::Value>(0usize) {
            if let srrldb::types::Value::Array(arr) = v {
                if let Some(row) = arr.first() {
                    if let srrldb::types::Value::Object(obj) = row {
                        if let Some(srrldb::types::Value::Number(n)) = obj.get("len") {
                            let existing_dim = n.to_int().unwrap_or(0) as u16;
                            if existing_dim > 0 && existing_dim != self.dimension {
                                tracing::error!(
                                    expected = self.dimension,
                                    actual = existing_dim,
                                    "Cache embedding dimension mismatch! Purging incompatible cache."
                                );
                                self.flush_all().await?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Synthetic ANN query to eager-load the HNSW index.
    ///
    /// Only called from `persistent()`. Ephemeral stores start empty, so
    /// there is nothing to warm up.
    #[cfg(feature = "cache-persistent")]
    async fn warmup(&self) -> Result<(), CacheError> {
        let count = self.entry_count().await?;

        if count > 0 {
            // Random unit vector for warmup
            let mut rng = rand::rng();
            let mut dummy = vec![0.0f32; self.dimension as usize];
            for x in &mut dummy {
                *x = rng.random_range(-1.0..1.0);
            }
            let mag = dummy.iter().map(|x| x * x).sum::<f32>().sqrt();
            if mag > 1e-6 {
                for x in &mut dummy {
                    *x /= mag;
                }
            } else {
                dummy[0] = 1.0;
            }

            let _ = self
                .db
                .query("SELECT * FROM cache WHERE embedding <|1, COSINE|> $vec LIMIT 1")
                .bind(("vec", dummy))
                .await;

            tracing::info!("HNSW index warmup complete");
        }
        Ok(())
    }

    /// Look up a prompt in the cache using two-tier resolution.
    pub async fn lookup(
        &self,
        exact_hash: &str,
        embedding: Option<&[f32]>,
        metadata: &CacheMetadata,
        threshold: f32,
        ttl_secs: u64,
    ) -> Result<CacheLookupResult, CacheError> {
        // Tier 1: Exact match
        if let Some(entry) = self.lookup_exact(exact_hash, ttl_secs).await? {
            let info = CacheHitInfo {
                kind: CacheHitKind::Exact,
                score: 1.0,
                threshold,
                metadata: metadata.clone(),
                hash_version: entry.hash_version.clone(),
            };
            return Ok(CacheLookupResult::Hit(entry, info));
        }

        // Tier 2: ANN vector search
        if let Some(emb) = embedding {
            if let Some((entry, score)) = self
                .lookup_approximate(emb, metadata, threshold, ttl_secs)
                .await?
            {
                let info = CacheHitInfo {
                    kind: CacheHitKind::Semantic,
                    score,
                    threshold,
                    metadata: metadata.clone(),
                    hash_version: entry.hash_version.clone(),
                };
                return Ok(CacheLookupResult::Hit(entry, info));
            }
        }

        Ok(CacheLookupResult::Miss)
    }

    /// Look up a cache entry by exact SHA-256 hash.
    ///
    /// **Note on metadata filtering:** The `exact_hash` already includes model,
    /// temperature, messages (including system messages), max_tokens, tools, and
    /// tool_choice. Separate system_prompt_hash / tool_definitions_hash filtering
    /// is therefore unnecessary here — those are used for the approximate (ANN)
    /// lookup path instead, where the embedding doesn't capture all of those fields.
    async fn lookup_exact(
        &self,
        hash: &str,
        ttl_secs: u64,
    ) -> Result<Option<CacheEntry>, CacheError> {
        let sql = "SELECT * FROM cache WHERE exact_hash = $hash AND created_at > time::now() - duration::from_secs($ttl) LIMIT 1";
        let mut response = self
            .db
            .query(sql)
            .bind(("hash", hash.to_string()))
            .bind(("ttl", ttl_secs))
            .await
            .lookup_err()?;

        let val: srrldb::types::Value = response.take(0usize).lookup_err()?;
        if let srrldb::types::Value::Array(vec) = val {
            if let Some(item) = vec.into_iter().next() {
                let entry = CacheEntry::from_value(item)
                    .map_err(|e| CacheError::Serialization(e.to_string()))?;
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    async fn lookup_approximate(
        &self,
        embedding: &[f32],
        metadata: &CacheMetadata,
        threshold: f32,
        ttl_secs: u64,
    ) -> Result<Option<(CacheEntry, f32)>, CacheError> {
        self.validate_vector(Some(embedding))?;

        let sql = "SELECT *, vector::similarity::cosine(embedding, $vec) AS score
                   FROM cache
                   WHERE embedding <|10, COSINE|> $vec
                     AND model = $model
                     AND system_prompt_hash = $sys_hash
                     AND tool_definitions_hash = $tool_hash
                     AND created_at > time::now() - duration::from_secs($ttl)
                   ORDER BY score DESC LIMIT 1";

        let mut response = self
            .db
            .query(sql)
            .bind(("vec", embedding.to_vec()))
            .bind(("model", metadata.model.clone()))
            .bind(("sys_hash", metadata.system_prompt_hash.clone()))
            .bind(("tool_hash", metadata.tool_definitions_hash.clone()))
            .bind(("ttl", ttl_secs))
            .await
            .lookup_err()?;

        let val: srrldb::types::Value = response.take(0usize).lookup_err()?;
        let entry_with_score = if let srrldb::types::Value::Array(vec) = val {
            vec.into_iter().next()
        } else {
            None
        };

        if let Some(val) = entry_with_score {
            let score = match val.get("score") {
                srrldb::types::Value::Number(n) => n.to_f64().unwrap_or(0.0) as f32,
                _ => 0.0,
            };
            if score >= threshold {
                let entry: CacheEntry = CacheEntry::from_value(val)
                    .map_err(|e| CacheError::Serialization(e.to_string()))?;
                return Ok(Some((entry, score)));
            }
        }

        Ok(None)
    }

    pub async fn insert(
        &self,
        entry: &CacheEntry,
        metadata: &CacheMetadata,
    ) -> Result<(), CacheError> {
        self.validate_vector(entry.embedding.as_deref())?;

        let sql = "INSERT INTO cache {
            exact_hash: $exact_hash,
            model: $model,
            system_prompt_hash: $sys_hash,
            tool_definitions_hash: $tool_hash,
            semantic_text: $sem_text,
            embedding: $emb,
            request_json: $req_json,
            response_json: $resp_json,
            hash_version: $hash_ver,
            temperature: $temp,
            confidence: $conf,
            created_at: time::now(),
            hit_count: 0
        } ON DUPLICATE KEY UPDATE
            response_json = $resp_json,
            embedding = $emb,
            hit_count += 1,
            system_prompt_hash = $sys_hash,
            tool_definitions_hash = $tool_hash,
            temperature = $temp,
            confidence = $conf,
            hash_version = $hash_ver";

        self.db
            .query(sql)
            .bind(("exact_hash", entry.exact_hash.clone()))
            .bind(("model", entry.model.clone()))
            .bind(("sys_hash", metadata.system_prompt_hash.clone()))
            .bind(("tool_hash", metadata.tool_definitions_hash.clone()))
            .bind(("sem_text", entry.semantic_text.clone()))
            .bind(("emb", entry.embedding.clone()))
            .bind(("req_json", entry.request_json.clone()))
            .bind(("resp_json", entry.response_json.clone()))
            .bind(("hash_ver", self.hash_version.clone()))
            .bind(("temp", metadata.temperature))
            .bind(("conf", metadata.confidence))
            .await
            .insert_err()?;

        Ok(())
    }

    /// Insert or update a cache entry with stream events (replay cache).
    ///
    /// If an entry with matching `exact_hash` already exists, only the stream
    /// fields are updated. Otherwise a full entry is created.
    pub async fn insert_stream(
        &self,
        entry: &CacheEntry,
        metadata: &CacheMetadata,
        events: &[String],
    ) -> Result<(), CacheError> {
        self.validate_vector(entry.embedding.as_deref())?;

        let sql = "INSERT INTO cache {
            exact_hash: $exact_hash,
            model: $model,
            system_prompt_hash: $sys_hash,
            tool_definitions_hash: $tool_hash,
            semantic_text: $sem_text,
            embedding: $emb,
            request_json: $req_json,
            response_json: $resp_json,
            hash_version: $hash_ver,
            temperature: $temp,
            confidence: $conf,
            stream_events: $stream_events,
            stream_format: $stream_format,
            created_at: time::now(),
            hit_count: 0
        } ON DUPLICATE KEY UPDATE
            stream_events = $stream_events,
            stream_format = $stream_format,
            response_json = $resp_json";

        self.db
            .query(sql)
            .bind(("exact_hash", entry.exact_hash.clone()))
            .bind(("model", entry.model.clone()))
            .bind(("sys_hash", metadata.system_prompt_hash.clone()))
            .bind(("tool_hash", metadata.tool_definitions_hash.clone()))
            .bind(("sem_text", entry.semantic_text.clone()))
            .bind(("emb", entry.embedding.clone()))
            .bind(("req_json", entry.request_json.clone()))
            .bind(("resp_json", entry.response_json.clone()))
            .bind(("hash_ver", self.hash_version.clone()))
            .bind(("temp", metadata.temperature))
            .bind(("conf", metadata.confidence))
            .bind(("stream_events", events.to_vec()))
            .bind(("stream_format", "openai_sse_v1".to_string()))
            .await
            .insert_err()?;

        Ok(())
    }

    fn validate_vector(&self, embedding: Option<&[f32]>) -> Result<(), CacheError> {
        if let Some(vec) = embedding {
            if vec.len() != self.dimension as usize {
                return Err(CacheError::DimensionMismatch {
                    expected: self.dimension,
                    actual: vec.len(),
                });
            }
            for &f in vec {
                if f.is_nan() || f.is_infinite() {
                    return Err(CacheError::InsertFailed(
                        "Vector contains NaN or Infinite values".to_string(),
                    ));
                }
            }
            let mag = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            if (mag - 1.0).abs() > 1e-3 {
                return Err(CacheError::NotNormalized { magnitude: mag });
            }
        }
        Ok(())
    }

    pub async fn record_hit(&self, exact_hash: &str) -> Result<(), CacheError> {
        let sql =
            "UPDATE cache SET hit_count += 1, last_hit = time::now() WHERE exact_hash = $hash";
        self.db
            .query(sql)
            .bind(("hash", exact_hash.to_string()))
            .await
            .lookup_err()?;
        Ok(())
    }

    pub async fn evict_expired(&self, ttl_secs: u64) -> Result<u64, CacheError> {
        let sql = "DELETE FROM cache WHERE created_at < time::now() - duration::from_secs($ttl) RETURN BEFORE";
        let mut response = self
            .db
            .query(sql)
            .bind(("ttl", ttl_secs))
            .await
            .lookup_err()?;

        let removed: Vec<CacheEntry> = response.take_vec(0usize).lookup_err()?;
        Ok(removed.len() as u64)
    }

    pub async fn evict_lru(&self, max_entries: usize) -> Result<u64, CacheError> {
        let total = self.entry_count().await?;

        if total <= max_entries as u64 {
            return Ok(0);
        }

        let to_remove = total - max_entries as u64;
        // LRU: remove entries with oldest last_hit first. Entries that have never
        // been accessed (last_hit = NONE) are evicted before any accessed entry.
        // Uses idiomatic SurrealDB DELETE-from-subquery to avoid the id indirection.
        let sql = "DELETE (
            SELECT * FROM cache
            ORDER BY last_hit = NONE DESC, last_hit ASC, created_at ASC
            LIMIT $to_remove
        ) RETURN BEFORE";
        let mut response = self
            .db
            .query(sql)
            .bind(("to_remove", to_remove))
            .await
            .lookup_err()?;

        let removed: Vec<CacheEntry> = response.take_vec(0usize).lookup_err()?;
        Ok(removed.len() as u64)
    }

    pub async fn flush_all(&self) -> Result<(), CacheError> {
        self.db.query("DELETE FROM cache").await.lookup_err()?;
        Ok(())
    }

    pub async fn flush_model(&self, model: &str) -> Result<(), CacheError> {
        self.db
            .query("DELETE FROM cache WHERE model = $model")
            .bind(("model", model.to_string()))
            .await
            .lookup_err()?;
        Ok(())
    }

    pub async fn count(&self) -> Result<u64, CacheError> {
        self.entry_count().await
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Return the total number of cache entries.
    ///
    /// Extracted helper — the `SELECT count() … GROUP ALL` + parse pattern
    /// was previously duplicated in `count()`, `evict_lru()`, and `warmup()`.
    async fn entry_count(&self) -> Result<u64, CacheError> {
        let mut response = self
            .db
            .query("SELECT count() FROM cache GROUP ALL")
            .await
            .lookup_err()?;

        let val: srrldb::types::Value = response.take(0usize).lookup_err()?;
        if let srrldb::types::Value::Array(vec) = val {
            if let Some(row) = vec.first() {
                if let srrldb::types::Value::Object(obj) = row {
                    if let Some(srrldb::types::Value::Number(n)) = obj.get("count") {
                        return Ok(n.clone().to_int().map(|i| i as u64).unwrap_or(0));
                    }
                }
            }
        }
        Ok(0)
    }
}

#[cfg(test)]
#[cfg(feature = "cache-ephemeral")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_store_basic() -> Result<(), CacheError> {
        let store = CacheStore::ephemeral(3).await?;

        let metadata = CacheMetadata {
            model: "test-model".into(),
            system_prompt_hash: "sys-hash".into(),
            tool_definitions_hash: "tool-hash".into(),
            temperature: Some(0.0),
            confidence: None,
        };

        let embedding = vec![1.0, 0.0, 0.0]; // Normalized
        let entry = CacheEntry {
            exact_hash: "exact-hash-1".into(),
            model: "test-model".into(),
            system_prompt_hash: "sys-hash".into(),
            tool_definitions_hash: "tool-hash".into(),
            semantic_text: "hello world".into(),
            embedding: Some(embedding.clone()),
            request_json: "{}".into(),
            response_json: "{\"result\": \"ok\"}".into(),
            created_at: srrldb::types::Datetime::now(),
            hit_count: 0,
            last_hit: None,
            hash_version: "v1".into(),
            stream_events: None,
            stream_format: None,
        };

        store.insert(&entry, &metadata).await?;
        assert_eq!(store.count().await?, 1);

        // Exact hit
        let res = store
            .lookup("exact-hash-1", None, &metadata, 0.9, 3600)
            .await?;
        assert!(res.is_hit());

        // Semantic hit
        let query_embedding = vec![0.99, 0.01, 0.0]; // Very close
        let mag = query_embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        let query_embedding: Vec<f32> = query_embedding.into_iter().map(|x| x / mag).collect();

        let res = store
            .lookup(
                "different-hash",
                Some(&query_embedding),
                &metadata,
                0.9,
                3600,
            )
            .await?;
        assert!(res.is_hit());
        if let CacheLookupResult::Hit(_, info) = res {
            assert_eq!(info.kind, CacheHitKind::Semantic);
            assert!(info.score > 0.9);
        }

        // Semantic miss (threshold)
        let far_embedding = vec![0.0, 1.0, 0.0]; // Orthogonal
        let res = store
            .lookup("different-hash", Some(&far_embedding), &metadata, 0.9, 3600)
            .await?;
        assert!(!res.is_hit());

        // Metadata mismatch
        let mut diff_metadata = metadata.clone();
        diff_metadata.system_prompt_hash = "different-sys".into();
        let res = store
            .lookup(
                "different-hash",
                Some(&query_embedding),
                &diff_metadata,
                0.9,
                3600,
            )
            .await?;
        assert!(!res.is_hit());

        Ok(())
    }

    #[tokio::test]
    async fn test_vector_validation() -> Result<(), CacheError> {
        let store = CacheStore::ephemeral(3).await?;

        let metadata = CacheMetadata {
            model: "test-model".into(),
            system_prompt_hash: "sys-hash".into(),
            tool_definitions_hash: "tool-hash".into(),
            temperature: Some(0.0),
            confidence: None,
        };

        let entry = CacheEntry {
            exact_hash: "h1".into(),
            model: "m1".into(),
            system_prompt_hash: "s1".into(),
            tool_definitions_hash: "t1".into(),
            semantic_text: "text".into(),
            embedding: Some(vec![1.0, 2.0]), // Wrong dimension
            request_json: "{}".into(),
            response_json: "{}".into(),
            created_at: srrldb::types::Datetime::now(),
            hit_count: 0,
            last_hit: None,
            hash_version: "v1".into(),
            stream_events: None,
            stream_format: None,
        };

        let res = store.insert(&entry, &metadata).await;
        assert!(matches!(res, Err(CacheError::DimensionMismatch { .. })));

        let entry_unnorm = CacheEntry {
            embedding: Some(vec![1.0, 1.0, 1.0]), // Magnitude sqrt(3) != 1
            stream_events: None,
            stream_format: None,
            ..entry
        };
        let res = store.insert(&entry_unnorm, &metadata).await;
        assert!(matches!(res, Err(CacheError::NotNormalized { .. })));

        Ok(())
    }

    #[tokio::test]
    async fn test_lru_eviction_uses_last_hit() -> Result<(), CacheError> {
        let store = CacheStore::ephemeral(3).await?;
        let metadata = CacheMetadata {
            model: "m".into(),
            system_prompt_hash: "s".into(),
            tool_definitions_hash: "t".into(),
            temperature: None,
            confidence: None,
        };

        // Insert 3 entries
        for i in 1..=3 {
            let entry = CacheEntry {
                exact_hash: format!("h{}", i),
                model: "m".into(),
                system_prompt_hash: "s".into(),
                tool_definitions_hash: "t".into(),
                semantic_text: "txt".into(),
                embedding: Some(vec![1.0, 0.0, 0.0]),
                request_json: "{}".into(),
                response_json: "{}".into(),
                created_at: srrldb::types::Datetime::now(),
                hit_count: 0,
                last_hit: None,
                hash_version: "v1".into(),
                stream_events: None,
                stream_format: None,
            };
            store.insert(&entry, &metadata).await?;
            // Small sleep to ensure different created_at
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Record hits for h1 and h2, but h1 hit is OLDER than h2 hit
        store.record_hit("h1").await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        store.record_hit("h2").await?;

        // Current state:
        // h3: last_hit=None (oldest by creation if we count hits first, but query says ORDER BY last_hit ASC, created_at ASC)
        // h1: last_hit=T1
        // h2: last_hit=T2 (T2 > T1)

        // In SurrealDB, NULLs usually come first in ASC order.
        // So h3 (last_hit IS NULL) should be first to go.

        store.evict_lru(2).await?;
        assert_eq!(store.count().await?, 2);
        assert!(store.lookup_exact("h3", 3600).await?.is_none());
        assert!(store.lookup_exact("h1", 3600).await?.is_some());
        assert!(store.lookup_exact("h2", 3600).await?.is_some());

        // Now hit h1 again, making it NEWER than h2
        store.record_hit("h1").await?;
        // Evict one more
        store.evict_lru(1).await?;
        assert_eq!(store.count().await?, 1);
        // h2 should be gone as its last_hit is now the oldest
        assert!(store.lookup_exact("h2", 3600).await?.is_none());
        assert!(store.lookup_exact("h1", 3600).await?.is_some());

        Ok(())
    }

    #[cfg(feature = "cache-persistent")]
    #[tokio::test]
    async fn test_compatibility_guard_purges_on_dimension_mismatch() -> Result<(), CacheError> {
        let temp = tempfile::tempdir().map_err(|e| CacheError::InitFailed(e.to_string()))?;
        let path = temp.path().join("gaud.cache");
        let path_str = path.to_str().unwrap();

        // 1. Initialize with dimension 3 and insert an entry
        {
            let store = CacheStore::persistent(path_str, 3).await?;
            let metadata = CacheMetadata {
                model: "m".into(),
                system_prompt_hash: "s".into(),
                tool_definitions_hash: "t".into(),
                temperature: None,
                confidence: None,
            };
            let entry = CacheEntry {
                exact_hash: "h".into(),
                model: "m".into(),
                system_prompt_hash: "s".into(),
                tool_definitions_hash: "t".into(),
                semantic_text: "txt".into(),
                embedding: Some(vec![1.0, 0.0, 0.0]),
                request_json: "{}".into(),
                response_json: "{}".into(),
                created_at: srrldb::types::Datetime::now(),
                hit_count: 0,
                last_hit: None,
                hash_version: "v1".into(),
                stream_events: None,
                stream_format: None,
            };
            store.insert(&entry, &metadata).await?;
            assert_eq!(store.count().await?, 1);
        }

        // 2. Re-initialize with dimension 4. Compatibility guard should purge.
        {
            let store = CacheStore::persistent(path_str, 4).await?;
            assert_eq!(
                store.count().await?,
                0,
                "Cache should have been purged due to dimension mismatch"
            );
        }

        Ok(())
    }
}
