use surrealdb::engine::local::Db;
#[cfg(feature = "cache-persistent")]
use surrealdb::engine::local::RocksDb;
#[cfg(feature = "cache-ephemeral")]
use surrealdb::engine::local::Mem;
use surrealdb::Surreal;
use chrono::Utc;
use serde_json;

use crate::cache::types::{
    CacheEntry, CacheError, CacheHitInfo, CacheHitKind, CacheLookupResult, CacheMetadata,
};

/// A thin, vector-aware cache layer over an embedded SurrealDB instance.
///
/// `CacheStore` provides two-tier lookup (exact hash → ANN vector search)
/// for LLM prompt/response pairs. All storage, indexing, and transaction
/// management is delegated to SurrealDB's embedded engine.
pub struct CacheStore {
    db: Surreal<Db>,
    dimension: u16,
    hash_version: String,
}

impl CacheStore {
    /// Initialize a persistent semantic cache backed by RocksDB.
    #[cfg(feature = "cache-persistent")]
    pub async fn persistent(path: &str, dimension: u16) -> Result<Self, CacheError> {
        let db = Surreal::new::<RocksDb>(path)
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
        let db = Surreal::new::<Mem>(())
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
        self.db.use_ns("gaud").use_db("cache").await
            .map_err(|e| CacheError::SchemaFailed(e.to_string()))?;

        // 1. Schema versioning
        self.db.query("DEFINE TABLE IF NOT EXISTS schema_version SCHEMAFULL;
                       DEFINE FIELD IF NOT EXISTS version ON schema_version TYPE int;
                       DEFINE FIELD IF NOT EXISTS applied_at ON schema_version TYPE datetime DEFAULT time::now();")
            .await
            .map_err(|e| CacheError::SchemaFailed(e.to_string()))?;

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

            DEFINE INDEX IF NOT EXISTS idx_prompt_hash ON cache FIELDS exact_hash UNIQUE;
            DEFINE INDEX IF NOT EXISTS hnsw_embedding ON cache FIELDS embedding 
                HNSW DIMENSION {dim} DIST COSINE;
            "#,
            dim = self.dimension
        );

        self.db.query(schema).await
            .map_err(|e| CacheError::SchemaFailed(e.to_string()))?;

        Ok(())
    }

    /// Synthetic ANN query to eager-load the HNSW index.
    async fn warmup(&self) -> Result<(), CacheError> {
        let count: Option<u64> = self.db.query("SELECT count() FROM cache GROUP ALL")
            .await
            .map_err(|e| CacheError::LookupFailed(e.to_string()))?
            .take(0)
            .map(|v: serde_json::Value| v["count"].as_u64().unwrap_or(0));

        if count.unwrap_or(0) > 0 {
            let mut dummy = vec![0.0f32; self.dimension as usize];
            dummy[0] = 1.0; // Random unit-ish vector
            
            let _ = self.db.query("SELECT * FROM cache WHERE embedding <|1, COSINE|> $vec LIMIT 1")
                .bind(("vec", &dummy))
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
                hash_version: self.hash_version.clone(),
            };
            return Ok(CacheLookupResult::Hit(entry, info));
        }

        // Tier 2: ANN vector search
        if let Some(emb) = embedding {
            if let Some((entry, score)) = self.lookup_approximate(emb, metadata, threshold, ttl_secs).await? {
                let info = CacheHitInfo {
                    kind: CacheHitKind::Approximate,
                    score,
                    threshold,
                    metadata: metadata.clone(),
                    hash_version: self.hash_version.clone(),
                };
                return Ok(CacheLookupResult::Hit(entry, info));
            }
        }

        Ok(CacheLookupResult::Miss)
    }

    async fn lookup_exact(&self, hash: &str, ttl_secs: u64) -> Result<Option<CacheEntry>, CacheError> {
        let sql = "SELECT * FROM cache WHERE exact_hash = $hash AND created_at > time::now() - $ttl LIMIT 1";
        let mut response = self.db.query(sql)
            .bind(("hash", hash))
            .bind(("ttl", format!("{}s", ttl_secs)))
            .await
            .map_err(|e| CacheError::LookupFailed(e.to_string()))?;
        
        Ok(response.take(0).map_err(|e| CacheError::LookupFailed(e.to_string()))?)
    }

    async fn lookup_approximate(
        &self,
        embedding: &[f32],
        metadata: &CacheMetadata,
        threshold: f32,
        ttl_secs: u64,
    ) -> Result<Option<(CacheEntry, f32)>, CacheError> {
        let sql = "SELECT *, vector::similarity::cosine(embedding, $vec) AS score 
                   FROM cache 
                   WHERE embedding <|10, COSINE|> $vec 
                     AND model = $model 
                     AND system_prompt_hash = $sys_hash
                     AND tool_definitions_hash = $tool_hash
                     AND created_at > time::now() - $ttl 
                   ORDER BY score DESC LIMIT 1";

        let mut response = self.db.query(sql)
            .bind(("vec", embedding))
            .bind(("model", &metadata.model))
            .bind(("sys_hash", &metadata.system_prompt_hash))
            .bind(("tool_hash", &metadata.tool_definitions_hash))
            .bind(("ttl", format!("{}s", ttl_secs)))
            .await
            .map_err(|e| CacheError::LookupFailed(e.to_string()))?;

        let entry_with_score: Option<serde_json::Value> = response.take(0).map_err(|e| CacheError::LookupFailed(e.to_string()))?;
        
        if let Some(val) = entry_with_score {
            let score = val["score"].as_f64().unwrap_or(0.0) as f32;
            if score >= threshold {
                let entry: CacheEntry = serde_json::from_value(val).map_err(|e| CacheError::Serialization(e.to_string()))?;
                return Ok(Some((entry, score)));
            }
        }

        Ok(None)
    }

    pub async fn insert(&self, entry: &CacheEntry, metadata: &CacheMetadata) -> Result<(), CacheError> {
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
            response_json = $input.response_json,
            embedding = $input.embedding,
            created_at = time::now(),
            hit_count = 0";

        self.db.query(sql)
            .bind(("exact_hash", &entry.exact_hash))
            .bind(("model", &entry.model))
            .bind(("sys_hash", &metadata.system_prompt_hash))
            .bind(("tool_hash", &metadata.tool_definitions_hash))
            .bind(("sem_text", &entry.semantic_text))
            .bind(("emb", &entry.embedding))
            .bind(("req_json", &entry.request_json))
            .bind(("resp_json", &entry.response_json))
            .bind(("hash_ver", &self.hash_version))
            .bind(("temp", metadata.temperature))
            .bind(("conf", metadata.confidence))
            .await
            .map_err(|e| CacheError::InsertFailed(e.to_string()))?;

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
                    return Err(CacheError::InsertFailed("Vector contains NaN or Infinite values".to_string()));
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
        let sql = "UPDATE cache SET hit_count += 1, last_hit = time::now() WHERE exact_hash = $hash";
        self.db.query(sql)
            .bind(("hash", exact_hash))
            .await
            .map_err(|e| CacheError::LookupFailed(e.to_string()))?;
        Ok(())
    }

    pub async fn evict_expired(&self, ttl_secs: u64) -> Result<u64, CacheError> {
        let sql = "DELETE FROM cache WHERE created_at < time::now() - $ttl RETURN BEFORE";
        let mut response = self.db.query(sql)
            .bind(("ttl", format!("{}s", ttl_secs)))
            .await
            .map_err(|e| CacheError::LookupFailed(e.to_string()))?;
        
        let removed: Vec<CacheEntry> = response.take(0).map_err(|e| CacheError::LookupFailed(e.to_string()))?;
        Ok(removed.len() as u64)
    }

    pub async fn evict_lru(&self, max_entries: usize) -> Result<u64, CacheError> {
        let count_sql = "SELECT count() FROM cache GROUP ALL";
        let mut response = self.db.query(count_sql).await.map_err(|e| CacheError::LookupFailed(e.to_string()))?;
        let total = response.take(0).map(|v: serde_json::Value| v["count"].as_u64().unwrap_or(0)).unwrap_or(0);

        if total <= max_entries as u64 {
            return Ok(0);
        }

        let to_remove = total - max_entries as u64;
        let sql = "DELETE FROM cache WHERE id IN (SELECT id FROM cache ORDER BY hit_count ASC, created_at ASC LIMIT $to_remove) RETURN BEFORE";
        let mut response = self.db.query(sql)
            .bind(("to_remove", to_remove))
            .await
            .map_err(|e| CacheError::LookupFailed(e.to_string()))?;
        
        let removed: Vec<CacheEntry> = response.take(0).map_err(|e| CacheError::LookupFailed(e.to_string()))?;
        Ok(removed.len() as u64)
    }

    pub async fn flush_all(&self) -> Result<(), CacheError> {
        self.db.query("DELETE FROM cache").await.map_err(|e| CacheError::LookupFailed(e.to_string()))?;
        Ok(())
    }

    pub async fn flush_model(&self, model: &str) -> Result<(), CacheError> {
        self.db.query("DELETE FROM cache WHERE model = $model")
            .bind(("model", model))
            .await
            .map_err(|e| CacheError::LookupFailed(e.to_string()))?;
        Ok(())
    }

        pub async fn count(&self) -> Result<u64, CacheError> {

            let mut response = self.db.query("SELECT count() FROM cache GROUP ALL").await.map_err(|e| CacheError::LookupFailed(e.to_string()))?;

            Ok(response.take(0).map(|v: serde_json::Value| v["count"].as_u64().unwrap_or(0)).unwrap_or(0))

        }

    }

    

    #[cfg(test)]

    mod tests {

        use super::*;

        use crate::cache::types::{CacheEntry, CacheMetadata};

    

        #[tokio::test]

        #[cfg(feature = "cache-ephemeral")]

        async fn test_cache_store_basic() -> Result<(), CacheError> {

            let store = CacheStore::ephemeral(3).await?;

            

            let metadata = CacheMetadata {

                model: "test-model".into(),

                system_prompt_hash: "sys-hash".into(),

                tool_definitions_hash: "tool-hash".into(),

                temperature: Some(0.0),

                confidence: None,

            };

    

            let mut embedding = vec![1.0, 0.0, 0.0]; // Normalized

            let entry = CacheEntry {

                exact_hash: "exact-hash-1".into(),

                model: "test-model".into(),

                system_prompt_hash: "sys-hash".into(),

                tool_definitions_hash: "tool-hash".into(),

                semantic_text: "hello world".into(),

                embedding: Some(embedding.clone()),

                request_json: "{}".into(),

                response_json: "{\"result\": \"ok\"}".into(),

                created_at: "".into(),

                hit_count: 0,

                last_hit: None,

                hash_version: "v1".into(),

            };

    

            store.insert(&entry, &metadata).await?;

            assert_eq!(store.count().await?, 1);

    

            // Exact hit

            let res = store.lookup("exact-hash-1", None, &metadata, 0.9, 3600).await?;

            assert!(res.is_hit());

    

            // Semantic hit

            let query_embedding = vec![0.99, 0.01, 0.0]; // Very close

            let res = store.lookup("different-hash", Some(&query_embedding), &metadata, 0.9, 3600).await?;

            assert!(res.is_hit());

            if let CacheLookupResult::Hit(_, info) = res {

                assert_eq!(info.kind, CacheHitKind::Approximate);

                assert!(info.score > 0.9);

            }

    

            // Semantic miss (threshold)

            let far_embedding = vec![0.0, 1.0, 0.0]; // Orthogonal

            let res = store.lookup("different-hash", Some(&far_embedding), &metadata, 0.9, 3600).await?;

            assert!(!res.is_hit());

    

            // Metadata mismatch

            let mut diff_metadata = metadata.clone();

            diff_metadata.system_prompt_hash = "different-sys".into();

            let res = store.lookup("different-hash", Some(&query_embedding), &diff_metadata, 0.9, 3600).await?;

            assert!(!res.is_hit());

    

            Ok(())

        }

    

        #[tokio::test]

        #[cfg(feature = "cache-ephemeral")]

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

                created_at: "".into(),

                hit_count: 0,

                last_hit: None,

                hash_version: "v1".into(),

            };

    

            let res = store.insert(&entry, &metadata).await;

            assert!(matches!(res, Err(CacheError::DimensionMismatch { .. })));

    

            let entry_unnorm = CacheEntry {

                embedding: Some(vec![1.0, 1.0, 1.0]), // Magnitude sqrt(3) != 1

                ..entry

            };

            let res = store.insert(&entry_unnorm, &metadata).await;

            assert!(matches!(res, Err(CacheError::NotNormalized { .. })));

    

            Ok(())

        }

    }

    