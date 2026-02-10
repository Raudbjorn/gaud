use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Cache entry stored in SurrealDB
// ---------------------------------------------------------------------------

/// A cached request→response pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub exact_hash: String,
    pub model: String,
    pub semantic_text: String,
    pub embedding: Option<Vec<f32>>,
    pub request_json: String,
    pub response_json: String,
    pub created_at: String,
    pub hit_count: u64,
    pub last_hit: Option<String>,
}

// ---------------------------------------------------------------------------
// Lookup result
// ---------------------------------------------------------------------------

/// Outcome of a cache lookup.
#[derive(Debug, Clone)]
pub enum CacheLookupResult {
    /// Exact SHA-256 hash match.
    ExactHit(CacheEntry),
    /// Semantic (embedding KNN) match with cosine similarity score.
    SemanticHit { entry: CacheEntry, similarity: f32 },
    /// No match found.
    Miss,
}

impl CacheLookupResult {
    pub fn is_hit(&self) -> bool {
        !matches!(self, Self::Miss)
    }

    pub fn hit_kind(&self) -> Option<&'static str> {
        match self {
            Self::ExactHit(_) => Some("exact"),
            Self::SemanticHit { .. } => Some("semantic"),
            Self::Miss => None,
        }
    }

    pub fn into_entry(self) -> Option<CacheEntry> {
        match self {
            Self::ExactHit(e) | Self::SemanticHit { entry: e, .. } => Some(e),
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
    #[error("SurrealDB error: {0}")]
    Store(String),

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_result_miss() {
        let result = CacheLookupResult::Miss;
        assert!(!result.is_hit());
        assert!(result.hit_kind().is_none());
        assert!(result.into_entry().is_none());
    }

    #[test]
    fn test_lookup_result_exact_hit() {
        let entry = CacheEntry {
            exact_hash: "abc123".into(),
            model: "gpt-4".into(),
            semantic_text: "hello".into(),
            embedding: None,
            request_json: "{}".into(),
            response_json: "{}".into(),
            created_at: "2025-01-01T00:00:00Z".into(),
            hit_count: 1,
            last_hit: None,
        };
        let result = CacheLookupResult::ExactHit(entry);
        assert!(result.is_hit());
        assert_eq!(result.hit_kind(), Some("exact"));
        assert!(result.into_entry().is_some());
    }

    #[test]
    fn test_lookup_result_semantic_hit() {
        let entry = CacheEntry {
            exact_hash: "abc123".into(),
            model: "gpt-4".into(),
            semantic_text: "hello".into(),
            embedding: Some(vec![0.1, 0.2]),
            request_json: "{}".into(),
            response_json: "{}".into(),
            created_at: "2025-01-01T00:00:00Z".into(),
            hit_count: 0,
            last_hit: None,
        };
        let result = CacheLookupResult::SemanticHit {
            entry,
            similarity: 0.95,
        };
        assert!(result.is_hit());
        assert_eq!(result.hit_kind(), Some("semantic"));
    }

    #[test]
    fn test_cache_stats() {
        let stats = CacheStats::new();
        stats.record_exact_hit();
        stats.record_exact_hit();
        stats.record_semantic_hit();
        stats.record_miss();

        let snap = stats.snapshot();
        assert_eq!(snap.hits_exact, 2);
        assert_eq!(snap.hits_semantic, 1);
        assert_eq!(snap.misses, 1);
        assert!((snap.hit_rate - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn test_cache_stats_zero() {
        let stats = CacheStats::new();
        let snap = stats.snapshot();
        assert_eq!(snap.hit_rate, 0.0);
    }

    #[test]
    fn test_cache_error_display() {
        let err = CacheError::Store("connection failed".into());
        assert!(err.to_string().contains("SurrealDB error"));
    }
}
