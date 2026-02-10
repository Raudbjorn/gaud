use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Cache entry stored in SurrealDB
// ---------------------------------------------------------------------------

/// A cached request→response pair.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
#[surreal(crate = "surrealdb::types")]
pub struct CacheEntry {
    pub exact_hash: String,
    pub model: String,
    pub system_prompt_hash: String,
    pub tool_definitions_hash: String,
    pub semantic_text: String,
    pub embedding: Option<Vec<f32>>,
    pub request_json: String,
    pub response_json: String,
    pub created_at: surrealdb::types::Datetime,
    pub hit_count: u64,
    pub last_hit: Option<surrealdb::types::Datetime>,
    pub hash_version: String,
}

// ---------------------------------------------------------------------------
// Lookup result & Hit Info
// ---------------------------------------------------------------------------

/// Detailed information about a cache hit for explainability.
#[derive(Debug, Clone, Serialize)]
pub struct CacheHitInfo {
    pub kind: CacheHitKind,
    pub score: f32,
    pub threshold: f32,
    pub metadata: CacheMetadata,
    pub hash_version: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CacheHitKind {
    Exact,
    Semantic,
}

/// Metadata attached to cached entries for validation and analysis.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
#[surreal(crate = "surrealdb::types")]
pub struct CacheMetadata {
    pub model: String,
    pub system_prompt_hash: String,
    pub tool_definitions_hash: String,
    pub temperature: Option<f32>,
    pub confidence: Option<f32>,
}

/// Outcome of a cache lookup.
#[derive(Debug, Clone)]
pub enum CacheLookupResult {
    /// Cache hit with detailed info.
    Hit(CacheEntry, CacheHitInfo),
    /// No match found.
    Miss,
}

impl CacheLookupResult {
    pub fn is_hit(&self) -> bool {
        matches!(self, Self::Hit(_, _))
    }

    pub fn hit_kind_str(&self) -> Option<&'static str> {
        match self {
            Self::Hit(_, info) => match info.kind {
                CacheHitKind::Exact => Some("exact"),
                CacheHitKind::Semantic => Some("semantic"),
            },
            Self::Miss => None,
        }
    }

    pub fn into_entry(self) -> Option<CacheEntry> {
        match self {
            Self::Hit(entry, _) => Some(entry),
            Self::Miss => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Stats (atomic counters, lock-free)
// ---------------------------------------------------------------------------

/// Runtime cache statistics.
pub struct CacheStats {
    pub hits_exact: AtomicU64,
    pub hits_semantic: AtomicU64,
    pub misses: AtomicU64,
}

impl CacheStats {
    pub fn new() -> Self {
        Self {
            hits_exact: AtomicU64::new(0),
            hits_semantic: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    pub fn record_exact_hit(&self) {
        self.hits_exact.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_semantic_hit(&self) {
        self.hits_semantic.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> CacheStatsSnapshot {
        let exact = self.hits_exact.load(Ordering::Relaxed);
        let semantic = self.hits_semantic.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = exact + semantic + misses;
        let hit_rate = if total > 0 {
            (exact + semantic) as f64 / total as f64
        } else {
            0.0
        };
        CacheStatsSnapshot {
            hits_exact: exact,
            hits_semantic: semantic,
            misses,
            hit_rate,
        }
    }
}

/// Serializable snapshot of cache statistics.
#[derive(Debug, Clone, Serialize)]
pub struct CacheStatsSnapshot {
    pub hits_exact: u64,
    pub hits_semantic: u64,
    pub misses: u64,
    pub hit_rate: f64,
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors specific to cache operations.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("Cache initialization failed: {0}")]
    InitFailed(String),

    #[error("Schema application failed: {0}")]
    SchemaFailed(String),

    #[error("Cache lookup failed: {0}")]
    LookupFailed(String),

    #[error("Cache insert failed: {0}")]
    InsertFailed(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Invalid embedding: expected dimension {expected}, got {actual}")]
    DimensionMismatch { expected: u16, actual: usize },

    #[error("Embedding vector is not normalized (magnitude: {magnitude:.4})")]
    NotNormalized { magnitude: f32 },

    #[error("Embedding API error: {0}")]
    Embedding(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Cache not initialized")]
    NotInitialized,
}

impl From<serde_json::Error> for CacheError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_stats() {
        let stats = CacheStats::new();
        stats.record_exact_hit();
        stats.record_semantic_hit();
        stats.record_miss();
        stats.record_exact_hit();

        let snap = stats.snapshot();
        assert_eq!(snap.hits_exact, 2);
        assert_eq!(snap.hits_semantic, 1);
        assert_eq!(snap.misses, 1);
        assert_eq!(snap.hit_rate, 0.75); // 3 hits / 4 total
    }

    #[test]
    fn test_lookup_result_helpers() {
        let entry = CacheEntry {
            exact_hash: "h".into(),
            model: "m".into(),
            system_prompt_hash: "s".into(),
            tool_definitions_hash: "t".into(),
            semantic_text: "txt".into(),
            embedding: None,
            request_json: "{}".into(),
            response_json: "{}".into(),
            created_at: surrealdb::types::Datetime::now(),
            hit_count: 0,
            last_hit: None,
            hash_version: "v1".into(),
        };

        let info = CacheHitInfo {
            kind: CacheHitKind::Exact,
            score: 1.0,
            threshold: 0.9,
            metadata: CacheMetadata {
                model: "m".into(),
                system_prompt_hash: "s".into(),
                tool_definitions_hash: "t".into(),
                temperature: None,
                confidence: None,
            },
            hash_version: "v1".into(),
        };

        let res = CacheLookupResult::Hit(entry.clone(), info);
        assert!(res.is_hit());
        assert_eq!(res.hit_kind_str(), Some("exact"));

        let unwrapped = res.into_entry();
        assert!(unwrapped.is_some());
        assert_eq!(unwrapped.unwrap().exact_hash, "h");

        let miss = CacheLookupResult::Miss;
        assert!(!miss.is_hit());
        assert_eq!(miss.hit_kind_str(), None);
        assert!(miss.into_entry().is_none());
    }
}
