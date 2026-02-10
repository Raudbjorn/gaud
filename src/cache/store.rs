use std::collections::BTreeMap;

use surrealdb_core::dbs::Session;
use surrealdb_core::kvs::Datastore;
use surrealdb_types::{Number as SurrealNumber, Value as SurrealValue_, Variables};

use crate::cache::types::{CacheEntry, CacheError};

// Re-alias for clarity within this module.
type Val = SurrealValue_;

// ---------------------------------------------------------------------------
// CacheStore -- SurrealDB Datastore wrapper
// ---------------------------------------------------------------------------

/// Low-level SurrealDB wrapper for cache entry CRUD.
pub struct CacheStore {
    ds: Datastore,
    session: Session,
}

impl CacheStore {
    /// Create a new in-memory SurrealDB datastore and initialize the schema.
    pub async fn new(embedding_dimension: u16, hnsw_m: u8, hnsw_efc: u16) -> Result<Self, CacheError> {
        let ds = Datastore::new("memory")
            .await
            .map_err(|e| CacheError::Store(format!("Failed to create datastore: {e}")))?;

        let session = Session::owner().with_ns("gaud").with_db("cache");

        let store = Self { ds, session };
        store.init_schema(embedding_dimension, hnsw_m, hnsw_efc).await?;
        Ok(store)
    }

    /// Initialize the SurrealQL schema (tables, fields, indexes).
    async fn init_schema(
        &self,
        dim: u16,
        m: u8,
        efc: u16,
    ) -> Result<(), CacheError> {
        let schema = format!(
            r#"
            DEFINE NAMESPACE IF NOT EXISTS gaud;
            DEFINE DATABASE IF NOT EXISTS cache;

            DEFINE TABLE IF NOT EXISTS entries SCHEMAFULL;
            DEFINE FIELD exact_hash    ON TABLE entries TYPE string;
            DEFINE FIELD model         ON TABLE entries TYPE string;
            DEFINE FIELD semantic_text  ON TABLE entries TYPE string;
            DEFINE FIELD embedding      ON TABLE entries FLEXIBLE TYPE option<array>;
            DEFINE FIELD request_json   ON TABLE entries TYPE string;
            DEFINE FIELD response_json  ON TABLE entries TYPE string;
            DEFINE FIELD created_at     ON TABLE entries TYPE datetime DEFAULT time::now();
            DEFINE FIELD hit_count      ON TABLE entries TYPE int DEFAULT 0;
            DEFINE FIELD last_hit       ON TABLE entries TYPE option<datetime>;

            DEFINE INDEX idx_exact   ON TABLE entries FIELDS exact_hash UNIQUE;
            DEFINE INDEX idx_model   ON TABLE entries FIELDS model;
            DEFINE INDEX idx_created ON TABLE entries FIELDS created_at;

            DEFINE INDEX idx_embedding ON TABLE entries
                FIELDS embedding
                HNSW DIMENSION {dim} DIST COSINE TYPE F32 EFC {efc} M {m};
            "#
        );

        self.execute(&schema).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Exact-match operations
    // -----------------------------------------------------------------------

    /// Look up a cache entry by exact SHA-256 hash.
    /// Adds TTL filtering so expired entries are never returned.
    pub async fn lookup_exact(
        &self,
        hash: &str,
        ttl_secs: u64,
    ) -> Result<Option<CacheEntry>, CacheError> {
        let sql = format!(
            "SELECT * FROM entries WHERE exact_hash = $hash \
             AND created_at > time::now() - {ttl_secs}s LIMIT 1"
        );
        let vars = self.vars([("hash", Val::String(hash.to_string()))]);
        let results = self.execute_with_vars(&sql, vars).await?;
        self.parse_single_entry(results)
    }

    /// Insert a new cache entry (or update if exact_hash already exists).
    pub async fn insert(&self, entry: &CacheEntry) -> Result<(), CacheError> {
        let sql = r#"
            INSERT INTO entries {
                exact_hash: $exact_hash,
                model: $model,
                semantic_text: $semantic_text,
                embedding: $embedding,
                request_json: $request_json,
                response_json: $response_json,
                hit_count: 0,
                created_at: time::now(),
                last_hit: NONE
            } ON DUPLICATE KEY UPDATE
                response_json = $input.response_json,
                embedding = $input.embedding,
                created_at = time::now(),
                hit_count = 0
        "#;

        let embedding_val = match &entry.embedding {
            Some(v) => Val::Array(
                v.iter()
                    .map(|f| Val::Number(SurrealNumber::Float(*f as f64)))
                    .collect::<Vec<Val>>()
                    .into(),
            ),
            None => Val::None,
        };

        let vars = self.vars([
            ("exact_hash", Val::String(entry.exact_hash.clone())),
            ("model", Val::String(entry.model.clone())),
            ("semantic_text", Val::String(entry.semantic_text.clone())),
            ("embedding", embedding_val),
            ("request_json", Val::String(entry.request_json.clone())),
            ("response_json", Val::String(entry.response_json.clone())),
        ]);

        self.execute_with_vars(sql, vars).await?;
        Ok(())
    }

    /// Increment the hit counter and update last_hit timestamp for an entry.
    pub async fn record_hit(&self, hash: &str) -> Result<(), CacheError> {
        let sql = r#"
            UPDATE entries SET
                hit_count += 1,
                last_hit = time::now()
            WHERE exact_hash = $hash
        "#;
        let vars = self.vars([("hash", Val::String(hash.to_string()))]);
        self.execute_with_vars(sql, vars).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Semantic search (HNSW KNN)
    // -----------------------------------------------------------------------

    /// KNN search for semantically similar entries by embedding vector.
    ///
    /// Returns the best entry for the given model with cosine similarity >= threshold.
    /// Uses in-query WHERE filtering so the planner handles condition evaluation
    /// during KNN traversal.
    pub async fn lookup_semantic(
        &self,
        embedding: &[f32],
        model: &str,
        k: usize,
        threshold: f32,
        ttl_secs: u64,
    ) -> Result<Option<(CacheEntry, f32)>, CacheError> {
        let embedding_val = Val::Array(
            embedding
                .iter()
                .map(|f| Val::Number(SurrealNumber::Float(*f as f64)))
                .collect::<Vec<Val>>()
                .into(),
        );

        let sql = format!(
            "SELECT *, vector::similarity::cosine(embedding, $vec) AS score \
             FROM entries \
             WHERE embedding <|{k},COSINE|> $vec \
             AND model = $model \
             AND created_at > time::now() - {ttl_secs}s \
             ORDER BY score DESC \
             LIMIT {k}"
        );

        let vars = self.vars([
            ("vec", embedding_val),
            ("model", Val::String(model.to_string())),
        ]);

        let results = self.execute_with_vars(&sql, vars).await?;

        // Find the best match above threshold.
        for result in results {
            match result.output() {
                Ok(Val::Array(arr)) => {
                    for val in arr.iter() {
                        if let Some(entry) = self.value_to_entry(val) {
                            let score = self.extract_float(val, "score");
                            let similarity = score as f32;
                            if similarity >= threshold {
                                return Ok(Some((entry, similarity)));
                            }
                        }
                    }
                }
                _ => continue,
            }
        }

        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Eviction / management
    // -----------------------------------------------------------------------

    /// Delete entries older than `ttl_secs` seconds.
    pub async fn evict_expired(&self, ttl_secs: u64) -> Result<u64, CacheError> {
        let sql = format!(
            "DELETE FROM entries WHERE created_at < time::now() - {ttl_secs}s RETURN BEFORE"
        );
        let results = self.execute(&sql).await?;
        let mut count = 0u64;
        for r in results {
            if let Ok(Val::Array(arr)) = r.output() {
                count += arr.len() as u64;
            }
        }
        Ok(count)
    }

    /// Delete entries exceeding `max_entries`, ordered by LRU (lowest
    /// hit_count first, then oldest created_at).
    pub async fn evict_lru(&self, max_entries: usize) -> Result<u64, CacheError> {
        let count_sql = "SELECT count() FROM entries GROUP ALL";
        let results = self.execute(count_sql).await?;

        let total = results
            .into_iter()
            .next()
            .and_then(|r| r.output().ok())
            .and_then(|v| {
                if let Val::Array(arr) = v {
                    arr.first().map(|row| self.extract_int(row, "count") as usize)
                } else {
                    None
                }
            })
            .unwrap_or(0);

        if total <= max_entries {
            return Ok(0);
        }

        let to_remove = total - max_entries;
        let sql = format!(
            "DELETE FROM entries WHERE id IN \
             (SELECT id FROM entries ORDER BY hit_count ASC, created_at ASC LIMIT {to_remove}) \
             RETURN BEFORE"
        );
        let results = self.execute(&sql).await?;
        let mut removed = 0u64;
        for r in results {
            if let Ok(Val::Array(arr)) = r.output() {
                removed += arr.len() as u64;
            }
        }
        Ok(removed)
    }

    /// Flush all cache entries.
    pub async fn flush_all(&self) -> Result<(), CacheError> {
        self.execute("DELETE FROM entries").await?;
        Ok(())
    }

    /// Flush entries for a specific model.
    pub async fn flush_model(&self, model: &str) -> Result<(), CacheError> {
        let vars = self.vars([("model", Val::String(model.to_string()))]);
        self.execute_with_vars("DELETE FROM entries WHERE model = $model", vars)
            .await?;
        Ok(())
    }

    /// Count total entries.
    pub async fn count(&self) -> Result<u64, CacheError> {
        let results = self.execute("SELECT count() FROM entries GROUP ALL").await?;
        let count = results
            .into_iter()
            .next()
            .and_then(|r| r.output().ok())
            .and_then(|v| {
                if let Val::Array(arr) = v {
                    arr.first().map(|row| self.extract_int(row, "count") as u64)
                } else {
                    None
                }
            })
            .unwrap_or(0);
        Ok(count)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Execute a SurrealQL statement without variables.
    async fn execute(
        &self,
        sql: &str,
    ) -> Result<Vec<surrealdb_core::dbs::QueryResult>, CacheError> {
        self.ds
            .execute(sql, &self.session, None)
            .await
            .map_err(|e| CacheError::Store(format!("Query failed: {e}")))
    }

    /// Execute a SurrealQL statement with variables.
    async fn execute_with_vars(
        &self,
        sql: &str,
        vars: Variables,
    ) -> Result<Vec<surrealdb_core::dbs::QueryResult>, CacheError> {
        self.ds
            .execute(sql, &self.session, Some(vars))
            .await
            .map_err(|e| CacheError::Store(format!("Query failed: {e}")))
    }

    /// Build a Variables map from key-value pairs.
    fn vars<const N: usize>(&self, pairs: [(&str, Val); N]) -> Variables {
        let map: BTreeMap<String, Val> = pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        Variables::from(map)
    }

    /// Parse a single CacheEntry from query results.
    fn parse_single_entry(
        &self,
        results: Vec<surrealdb_core::dbs::QueryResult>,
    ) -> Result<Option<CacheEntry>, CacheError> {
        for result in results {
            match result.output() {
                Ok(Val::Array(arr)) => {
                    if let Some(val) = arr.first() {
                        return Ok(self.value_to_entry(val));
                    }
                }
                Ok(_) => continue,
                Err(e) => return Err(CacheError::Store(format!("Query error: {e}"))),
            }
        }
        Ok(None)
    }

    /// Extract a string field from a SurrealDB Value (object row).
    fn extract_string(&self, val: &Val, field: &str) -> String {
        match &val[field] {
            Val::String(s) => s.clone(),
            _ => String::new(),
        }
    }

    /// Extract a float field from a SurrealDB Value (object row).
    fn extract_float(&self, val: &Val, field: &str) -> f64 {
        match &val[field] {
            Val::Number(n) => n.to_f64().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    /// Extract an integer field from a SurrealDB Value (object row).
    fn extract_int(&self, val: &Val, field: &str) -> i64 {
        match &val[field] {
            Val::Number(n) => n.to_int().unwrap_or(0),
            _ => 0,
        }
    }

    /// Convert a SurrealDB Value (object) into a CacheEntry.
    fn value_to_entry(&self, val: &Val) -> Option<CacheEntry> {
        let exact_hash = self.extract_string(val, "exact_hash");
        let model = self.extract_string(val, "model");
        let semantic_text = self.extract_string(val, "semantic_text");
        let request_json = self.extract_string(val, "request_json");
        let response_json = self.extract_string(val, "response_json");
        let created_at = match &val["created_at"] {
            Val::Datetime(dt) => dt.to_string(),
            Val::String(s) => s.clone(),
            _ => String::new(),
        };
        let hit_count = self.extract_int(val, "hit_count") as u64;
        let last_hit = match &val["last_hit"] {
            Val::None | Val::Null => None,
            Val::Datetime(dt) => Some(dt.to_string()),
            Val::String(s) => Some(s.clone()),
            _ => None,
        };

        let embedding = match &val["embedding"] {
            Val::Array(arr) => Some(
                arr.iter()
                    .filter_map(|v| match v {
                        Val::Number(n) => Some(n.to_f64().unwrap_or(0.0) as f32),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        };

        if exact_hash.is_empty() {
            return None;
        }

        Some(CacheEntry {
            exact_hash,
            model,
            semantic_text,
            embedding,
            request_json,
            response_json,
            created_at,
            hit_count,
            last_hit,
        })
    }
}
