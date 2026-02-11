//! Signature cache for thinking block continuity.
//!
//! This module provides a thread-safe cache for storing thinking signatures,
//! enabling signature recovery when clients (like Claude Code) strip custom fields.
//!
//! ## Overview
//!
//! Gemini 3+ models require `thoughtSignature` on tool calls and thinking blocks
//! for multi-turn conversations. However, Claude Code CLI strips non-standard fields
//! from API responses. This cache stores signatures so they can be restored in
//! subsequent requests.
//!
//! The cache also tracks model family ('claude' or 'gemini') for each signature,
//! enabling detection of cross-model conversations where signature compatibility
//! must be validated.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use antigravity_gate::convert::GLOBAL_SIGNATURE_CACHE;
//!
//! // Store a signature from a response
//! GLOBAL_SIGNATURE_CACHE.store_tool_signature("toolu_abc123", "sig...", "gemini");
//!
//! // Recover signature in subsequent request
//! if let Some(sig) = GLOBAL_SIGNATURE_CACHE.get_tool_signature("toolu_abc123") {
//!     // Use recovered signature
//! }
//! ```

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::constants::{
    ModelFamily, GEMINI_SKIP_SIGNATURE, MIN_SIGNATURE_LENGTH, SIGNATURE_CACHE_TTL,
};

/// Sentinel value used when signature cannot be recovered.
///
/// When a thinking signature is missing and cannot be recovered from cache,
/// this value can be used to tell Gemini to skip signature validation.
pub const SKIP_SIGNATURE_SENTINEL: &str = GEMINI_SKIP_SIGNATURE;

/// Maximum number of entries in each cache before eviction.
const MAX_CACHE_ENTRIES: usize = 1000;

/// Entry in the signature cache with timestamp for TTL expiry.
#[derive(Debug, Clone)]
struct CacheEntry {
    /// The cached signature value.
    signature: String,
    /// Model family that produced this signature.
    model_family: ModelFamily,
    /// When this entry was created.
    created_at: Instant,
}

impl CacheEntry {
    /// Create a new cache entry.
    fn new(signature: String, model_family: ModelFamily) -> Self {
        Self {
            signature,
            model_family,
            created_at: Instant::now(),
        }
    }

    /// Check if this entry has expired.
    fn is_expired(&self, ttl: Duration) -> bool {
        self.created_at.elapsed() > ttl
    }
}

/// Thread-safe cache for thinking signatures.
///
/// This cache stores two types of signatures:
/// 1. **Tool signatures**: Keyed by tool_use_id, used for function call continuity
/// 2. **Thinking signatures**: Keyed by first 100 chars of thinking text, used for
///    cross-model compatibility checking
///
/// Both caches use TTL-based expiry and LRU-style eviction when full.
#[derive(Debug)]
pub struct SignatureCache {
    /// Cache for tool_use_id -> signature mappings.
    tool_signatures: RwLock<HashMap<String, CacheEntry>>,
    /// Cache for thinking text -> signature mappings.
    thinking_signatures: RwLock<HashMap<String, CacheEntry>>,
    /// Time-to-live for cache entries.
    ttl: Duration,
}

impl SignatureCache {
    /// Create a new signature cache with the specified TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            tool_signatures: RwLock::new(HashMap::new()),
            thinking_signatures: RwLock::new(HashMap::new()),
            ttl,
        }
    }

    /// Create a new signature cache with default TTL (2 hours).
    pub fn with_default_ttl() -> Self {
        Self::new(SIGNATURE_CACHE_TTL)
    }

    /// Store a tool signature by tool_use_id.
    ///
    /// # Arguments
    ///
    /// * `tool_use_id` - The unique ID of the tool use
    /// * `signature` - The thoughtSignature to cache
    /// * `model_family` - The model family that produced this signature
    pub fn store_tool_signature(
        &self,
        tool_use_id: impl Into<String>,
        signature: impl Into<String>,
        model_family: ModelFamily,
    ) {
        let tool_use_id = tool_use_id.into();
        let signature = signature.into();

        if tool_use_id.is_empty() || signature.is_empty() {
            return;
        }

        let mut cache = self.tool_signatures.write().unwrap();

        // Evict expired entries if cache is getting full
        if cache.len() >= MAX_CACHE_ENTRIES {
            self.evict_expired(&mut cache);
        }

        // If still too full, remove oldest entries
        if cache.len() >= MAX_CACHE_ENTRIES {
            self.evict_oldest(&mut cache, MAX_CACHE_ENTRIES / 4);
        }

        cache.insert(tool_use_id, CacheEntry::new(signature, model_family));
    }

    /// Get a cached tool signature.
    ///
    /// Returns `None` if not found or expired.
    pub fn get_tool_signature(&self, tool_use_id: &str) -> Option<String> {
        if tool_use_id.is_empty() {
            return None;
        }

        let cache = self.tool_signatures.read().unwrap();
        cache.get(tool_use_id).and_then(|entry| {
            if entry.is_expired(self.ttl) {
                None
            } else {
                Some(entry.signature.clone())
            }
        })
    }

    /// Get a cached tool signature or the sentinel value.
    ///
    /// Returns the cached signature if found and not expired,
    /// otherwise returns the skip signature sentinel.
    pub fn get_tool_signature_or_sentinel(&self, tool_use_id: &str) -> String {
        self.get_tool_signature(tool_use_id)
            .unwrap_or_else(|| SKIP_SIGNATURE_SENTINEL.to_string())
    }

    /// Store a thinking signature.
    ///
    /// The signature is indexed by a cache key derived from the first 100 characters
    /// of the thinking text.
    ///
    /// # Arguments
    ///
    /// * `thinking_text` - The thinking content (first 100 chars used as key)
    /// * `signature` - The signature to cache
    /// * `model_family` - The model family that produced this signature
    pub fn store_thinking_signature(
        &self,
        thinking_text: &str,
        signature: impl Into<String>,
        model_family: ModelFamily,
    ) {
        let signature = signature.into();

        if signature.len() < MIN_SIGNATURE_LENGTH {
            return;
        }

        let key = Self::thinking_cache_key(thinking_text);
        if key.is_empty() {
            return;
        }

        let mut cache = self.thinking_signatures.write().unwrap();

        // Evict expired entries if cache is getting full
        if cache.len() >= MAX_CACHE_ENTRIES {
            self.evict_expired(&mut cache);
        }

        // If still too full, remove oldest entries
        if cache.len() >= MAX_CACHE_ENTRIES {
            self.evict_oldest(&mut cache, MAX_CACHE_ENTRIES / 4);
        }

        cache.insert(key, CacheEntry::new(signature, model_family));
    }

    /// Get a cached thinking signature.
    ///
    /// Returns `None` if not found or expired.
    pub fn get_thinking_signature(&self, thinking_text: &str) -> Option<String> {
        let key = Self::thinking_cache_key(thinking_text);
        if key.is_empty() {
            return None;
        }

        let cache = self.thinking_signatures.read().unwrap();
        cache.get(&key).and_then(|entry| {
            if entry.is_expired(self.ttl) {
                None
            } else {
                Some(entry.signature.clone())
            }
        })
    }

    /// Get the model family for a cached thinking signature.
    ///
    /// This is used for cross-model compatibility checking - signatures from
    /// one model family may not be valid for another.
    pub fn get_thinking_signature_family(&self, signature: &str) -> Option<ModelFamily> {
        if signature.len() < MIN_SIGNATURE_LENGTH {
            return None;
        }

        // Search by signature value (less efficient but necessary for this lookup)
        let cache = self.thinking_signatures.read().unwrap();
        for entry in cache.values() {
            if entry.signature == signature && !entry.is_expired(self.ttl) {
                return Some(entry.model_family);
            }
        }
        None
    }

    /// Check if a signature is compatible with the target model family.
    ///
    /// Returns `true` if:
    /// - The signature's family matches the target family
    /// - The signature's family is unknown (cold cache)
    /// - Targeting Claude (Claude validates its own signatures)
    ///
    /// Returns `false` if:
    /// - The signature is from a different family when targeting Gemini
    pub fn is_signature_compatible(&self, signature: &str, target_family: ModelFamily) -> bool {
        // Claude validates its own signatures, so we're lenient
        if target_family == ModelFamily::Claude {
            return true;
        }

        // For Gemini, check if signature originated from Gemini
        if let Some(sig_family) = self.get_thinking_signature_family(signature) {
            sig_family == target_family
        } else {
            // Unknown origin - conservative approach for Gemini
            // Return false to indicate incompatibility (Gemini is strict)
            false
        }
    }

    /// Clear all entries from the tool signature cache.
    pub fn clear_tool_signatures(&self) {
        let mut cache = self.tool_signatures.write().unwrap();
        cache.clear();
    }

    /// Clear all entries from the thinking signature cache.
    pub fn clear_thinking_signatures(&self) {
        let mut cache = self.thinking_signatures.write().unwrap();
        cache.clear();
    }

    /// Clear all caches.
    pub fn clear_all(&self) {
        self.clear_tool_signatures();
        self.clear_thinking_signatures();
    }

    /// Get the number of entries in the tool signature cache.
    pub fn tool_signature_count(&self) -> usize {
        self.tool_signatures.read().unwrap().len()
    }

    /// Get the number of entries in the thinking signature cache.
    pub fn thinking_signature_count(&self) -> usize {
        self.thinking_signatures.read().unwrap().len()
    }

    /// Generate a cache key from thinking text.
    ///
    /// Uses the first 100 characters to create a stable key.
    fn thinking_cache_key(thinking_text: &str) -> String {
        let chars: String = thinking_text.chars().take(100).collect();
        chars.trim().to_string()
    }

    /// Evict expired entries from a cache.
    fn evict_expired(&self, cache: &mut HashMap<String, CacheEntry>) {
        cache.retain(|_, entry| !entry.is_expired(self.ttl));
    }

    /// Evict the oldest N entries from a cache.
    fn evict_oldest(&self, cache: &mut HashMap<String, CacheEntry>, count: usize) {
        if cache.is_empty() || count == 0 {
            return;
        }

        // Sort by creation time and remove oldest
        let mut entries: Vec<_> = cache
            .iter()
            .map(|(k, v)| (k.clone(), v.created_at))
            .collect();
        entries.sort_by_key(|(_, created)| *created);

        for (key, _) in entries.into_iter().take(count) {
            cache.remove(&key);
        }
    }
}

impl Default for SignatureCache {
    fn default() -> Self {
        Self::with_default_ttl()
    }
}

/// Global signature cache instance.
///
/// This is a lazily-initialized global cache used by the conversion functions.
/// In most cases, you should use this rather than creating your own cache.
pub static GLOBAL_SIGNATURE_CACHE: std::sync::LazyLock<SignatureCache> =
    std::sync::LazyLock::new(SignatureCache::default);

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_store_and_get_tool_signature() {
        let cache = SignatureCache::with_default_ttl();

        cache.store_tool_signature("toolu_123", "sig_abc", ModelFamily::Gemini);

        assert_eq!(
            cache.get_tool_signature("toolu_123"),
            Some("sig_abc".to_string())
        );
        assert_eq!(cache.get_tool_signature("toolu_456"), None);
    }

    #[test]
    fn test_get_tool_signature_or_sentinel() {
        let cache = SignatureCache::with_default_ttl();

        cache.store_tool_signature("toolu_123", "sig_abc", ModelFamily::Gemini);

        assert_eq!(cache.get_tool_signature_or_sentinel("toolu_123"), "sig_abc");
        assert_eq!(
            cache.get_tool_signature_or_sentinel("toolu_missing"),
            SKIP_SIGNATURE_SENTINEL
        );
    }

    #[test]
    fn test_store_and_get_thinking_signature() {
        let cache = SignatureCache::with_default_ttl();

        let thinking_text = "Let me analyze this problem step by step...";
        let signature = "a".repeat(100); // Must be at least MIN_SIGNATURE_LENGTH

        cache.store_thinking_signature(thinking_text, &signature, ModelFamily::Claude);

        assert_eq!(
            cache.get_thinking_signature(thinking_text),
            Some(signature.clone())
        );
        assert_eq!(cache.get_thinking_signature("Different text"), None);
    }

    #[test]
    fn test_thinking_signature_family() {
        let cache = SignatureCache::with_default_ttl();

        let thinking_text = "Analyzing the user's request carefully...";
        let signature = "b".repeat(100);

        cache.store_thinking_signature(thinking_text, &signature, ModelFamily::Gemini);

        assert_eq!(
            cache.get_thinking_signature_family(&signature),
            Some(ModelFamily::Gemini)
        );
    }

    #[test]
    fn test_signature_compatibility() {
        let cache = SignatureCache::with_default_ttl();

        let thinking_text = "Processing the query...";
        let gemini_sig = "g".repeat(100);
        let claude_sig = "c".repeat(100);

        cache.store_thinking_signature(thinking_text, &gemini_sig, ModelFamily::Gemini);
        cache.store_thinking_signature("Other text", &claude_sig, ModelFamily::Claude);

        // Claude is lenient - accepts any signature
        assert!(cache.is_signature_compatible(&gemini_sig, ModelFamily::Claude));
        assert!(cache.is_signature_compatible(&claude_sig, ModelFamily::Claude));

        // Gemini is strict - only accepts Gemini signatures
        assert!(cache.is_signature_compatible(&gemini_sig, ModelFamily::Gemini));
        assert!(!cache.is_signature_compatible(&claude_sig, ModelFamily::Gemini));

        // Unknown signature - Gemini rejects, Claude accepts
        let unknown_sig = "u".repeat(100);
        assert!(!cache.is_signature_compatible(&unknown_sig, ModelFamily::Gemini));
        assert!(cache.is_signature_compatible(&unknown_sig, ModelFamily::Claude));
    }

    #[test]
    fn test_ttl_expiry() {
        // Use a very short TTL for testing
        let cache = SignatureCache::new(Duration::from_millis(50));

        cache.store_tool_signature("toolu_ttl", "sig_ttl", ModelFamily::Gemini);
        assert!(cache.get_tool_signature("toolu_ttl").is_some());

        // Wait for expiry
        thread::sleep(Duration::from_millis(100));

        assert!(cache.get_tool_signature("toolu_ttl").is_none());
    }

    #[test]
    fn test_thinking_cache_key() {
        // Short text uses full text
        let short = "Short thinking text";
        let key = SignatureCache::thinking_cache_key(short);
        assert_eq!(key, short);

        // Long text is truncated to 100 chars
        let long = "x".repeat(200);
        let key = SignatureCache::thinking_cache_key(&long);
        assert_eq!(key.len(), 100);

        // Whitespace is trimmed
        let padded = "  padded text  ";
        let key = SignatureCache::thinking_cache_key(padded);
        assert_eq!(key, "padded text");
    }

    #[test]
    fn test_empty_inputs() {
        let cache = SignatureCache::with_default_ttl();

        // Empty tool_use_id should be ignored
        cache.store_tool_signature("", "sig", ModelFamily::Gemini);
        assert!(cache.get_tool_signature("").is_none());

        // Empty signature should be ignored
        cache.store_tool_signature("toolu_empty", "", ModelFamily::Gemini);
        assert!(cache.get_tool_signature("toolu_empty").is_none());

        // Too-short signature for thinking should be ignored
        cache.store_thinking_signature("text", "short", ModelFamily::Gemini);
        assert!(cache.get_thinking_signature("text").is_none());
    }

    #[test]
    fn test_clear_caches() {
        let cache = SignatureCache::with_default_ttl();

        cache.store_tool_signature("toolu_1", "sig_1", ModelFamily::Gemini);
        cache.store_thinking_signature("thinking", "s".repeat(100), ModelFamily::Claude);

        assert_eq!(cache.tool_signature_count(), 1);
        assert_eq!(cache.thinking_signature_count(), 1);

        cache.clear_tool_signatures();
        assert_eq!(cache.tool_signature_count(), 0);
        assert_eq!(cache.thinking_signature_count(), 1);

        cache.clear_thinking_signatures();
        assert_eq!(cache.thinking_signature_count(), 0);
    }

    #[test]
    fn test_clear_all() {
        let cache = SignatureCache::with_default_ttl();

        cache.store_tool_signature("toolu_1", "sig_1", ModelFamily::Gemini);
        cache.store_thinking_signature("thinking", "s".repeat(100), ModelFamily::Claude);

        cache.clear_all();

        assert_eq!(cache.tool_signature_count(), 0);
        assert_eq!(cache.thinking_signature_count(), 0);
    }

    #[test]
    fn test_thread_safety() {
        let cache = std::sync::Arc::new(SignatureCache::with_default_ttl());
        let mut handles = vec![];

        // Spawn multiple threads that read and write concurrently
        for i in 0..10 {
            let cache_clone = cache.clone();
            handles.push(thread::spawn(move || {
                let id = format!("toolu_{}", i);
                let sig = format!("sig_{}", i);
                cache_clone.store_tool_signature(&id, &sig, ModelFamily::Gemini);
                thread::sleep(Duration::from_millis(1));
                cache_clone.get_tool_signature(&id)
            }));
        }

        // All threads should complete without panicking
        for handle in handles {
            let result = handle.join().unwrap();
            assert!(result.is_some());
        }
    }

    #[test]
    fn test_skip_signature_sentinel() {
        assert_eq!(SKIP_SIGNATURE_SENTINEL, "skip_thought_signature_validator");
    }

    #[test]
    fn test_global_cache() {
        // Just verify it can be accessed
        GLOBAL_SIGNATURE_CACHE.store_tool_signature(
            "global_test",
            "global_sig",
            ModelFamily::Gemini,
        );
        assert!(GLOBAL_SIGNATURE_CACHE
            .get_tool_signature("global_test")
            .is_some());

        // Clean up
        GLOBAL_SIGNATURE_CACHE.clear_all();
    }
}
