//! Retry logic and backoff strategies for provider requests.
//!
//! Implements exponential backoff, retry policies, and fallback model support
//! inspired by litellm's retry mechanisms.

use std::time::Duration;
use tracing::{debug, warn};

// MARK: - Constants

/// Default maximum number of retries.
const DEFAULT_MAX_RETRIES: u32 = 3;

/// Default initial backoff duration (1 second).
const DEFAULT_INITIAL_BACKOFF_MS: u64 = 1000;

/// Default maximum backoff duration (60 seconds).
const DEFAULT_MAX_BACKOFF_MS: u64 = 60_000;

/// Default backoff multiplier.
const DEFAULT_BACKOFF_MULTIPLIER: f64 = 2.0;

// MARK: - Retry Policy

/// Policy for retrying failed requests with exponential backoff.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Initial backoff duration in milliseconds.
    pub initial_backoff_ms: u64,
    /// Maximum backoff duration in milliseconds.
    pub max_backoff_ms: u64,
    /// Backoff multiplier for exponential backoff.
    pub backoff_multiplier: f64,
    /// Fallback models to try if primary model fails.
    pub fallback_models: Vec<String>,
    /// Whether to retry on rate limit errors.
    pub retry_on_rate_limit: bool,
    /// Whether to retry on timeout errors.
    pub retry_on_timeout: bool,
    /// Whether to retry on server errors (5xx).
    pub retry_on_server_error: bool,
}

impl RetryPolicy {
    /// Create a new retry policy with default settings.
    pub fn new() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            initial_backoff_ms: DEFAULT_INITIAL_BACKOFF_MS,
            max_backoff_ms: DEFAULT_MAX_BACKOFF_MS,
            backoff_multiplier: DEFAULT_BACKOFF_MULTIPLIER,
            fallback_models: Vec::new(),
            retry_on_rate_limit: true,
            retry_on_timeout: true,
            retry_on_server_error: true,
        }
    }

    /// Set maximum number of retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set initial backoff duration.
    pub fn with_initial_backoff(mut self, duration: Duration) -> Self {
        self.initial_backoff_ms = duration.as_millis() as u64;
        self
    }

    /// Set maximum backoff duration.
    pub fn with_max_backoff(mut self, duration: Duration) -> Self {
        self.max_backoff_ms = duration.as_millis() as u64;
        self
    }

    /// Set backoff multiplier.
    pub fn with_backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.backoff_multiplier = multiplier;
        self
    }

    /// Set fallback models.
    pub fn with_fallback_models(mut self, models: Vec<String>) -> Self {
        self.fallback_models = models;
        self
    }

    /// Calculate backoff duration for a given retry attempt.
    pub fn calculate_backoff(&self, attempt: u32) -> Duration {
        let backoff_ms = (self.initial_backoff_ms as f64
            * self.backoff_multiplier.powi(attempt as i32))
        .min(self.max_backoff_ms as f64) as u64;

        Duration::from_millis(backoff_ms)
    }

    /// Check if an error should be retried.
    pub fn should_retry(&self, error: &super::ProviderError, attempt: u32) -> bool {
        if attempt >= self.max_retries {
            return false;
        }

        match error {
            super::ProviderError::RateLimited { .. } => self.retry_on_rate_limit,
            super::ProviderError::Api { status, .. } => {
                // Retry on 5xx server errors
                if *status >= 500 && *status < 600 {
                    return self.retry_on_server_error;
                }
                // Retry on 429 (rate limit)
                if *status == 429 {
                    return self.retry_on_rate_limit;
                }
                // Retry on 408 (timeout)
                if *status == 408 {
                    return self.retry_on_timeout;
                }
                false
            }
            super::ProviderError::Http(e) => {
                // Retry on timeout errors
                if e.is_timeout() {
                    return self.retry_on_timeout;
                }
                // Retry on connection errors
                if e.is_connect() {
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// Get the next fallback model, if any.
    pub fn next_fallback_model(&self, attempt: u32) -> Option<&str> {
        let fallback_index = attempt.saturating_sub(1) as usize;
        self.fallback_models.get(fallback_index).map(|s| s.as_str())
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

// MARK: - Retry Executor

/// Execute a request with retry logic.
pub async fn execute_with_retry<F, Fut, T, E>(
    policy: &RetryPolicy,
    mut operation: F,
) -> Result<T, E>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut attempt = 0;

    loop {
        match operation(attempt).await {
            Ok(result) => {
                if attempt > 0 {
                    debug!(attempt = attempt, "Request succeeded after retry");
                }
                return Ok(result);
            }
            Err(error) => {
                attempt += 1;

                if attempt >= policy.max_retries {
                    warn!(
                        attempt = attempt,
                        max_retries = policy.max_retries,
                        error = %error,
                        "Max retries exceeded"
                    );
                    return Err(error);
                }

                let backoff = policy.calculate_backoff(attempt);
                warn!(
                    attempt = attempt,
                    backoff_ms = backoff.as_millis(),
                    error = %error,
                    "Request failed, retrying after backoff"
                );

                tokio::time::sleep(backoff).await;
            }
        }
    }
}

/// Execute a provider request with retry logic, using provider-specific
/// `retry_after` durations when available (e.g. from 429 headers).
pub async fn execute_provider_with_retry<F, Fut, T>(
    policy: &RetryPolicy,
    mut operation: F,
) -> Result<T, super::ProviderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, super::ProviderError>>,
{
    let mut attempt = 0u32;

    loop {
        match operation().await {
            Ok(result) => {
                if attempt > 0 {
                    debug!(attempt, "Provider request succeeded after retry");
                }
                return Ok(result);
            }
            Err(error) => {
                if !policy.should_retry(&error, attempt) {
                    return Err(error);
                }

                attempt += 1;

                // Prefer provider-supplied retry_after over calculated backoff.
                let backoff = error
                    .retry_after_duration()
                    .unwrap_or_else(|| policy.calculate_backoff(attempt));

                warn!(
                    attempt,
                    backoff_ms = backoff.as_millis() as u64,
                    error = %error,
                    "Provider request failed, retrying"
                );

                tokio::time::sleep(backoff).await;
            }
        }
    }
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = RetryPolicy::new();
        assert_eq!(policy.max_retries, DEFAULT_MAX_RETRIES);
        assert_eq!(policy.initial_backoff_ms, DEFAULT_INITIAL_BACKOFF_MS);
        assert_eq!(policy.max_backoff_ms, DEFAULT_MAX_BACKOFF_MS);
        assert_eq!(policy.backoff_multiplier, DEFAULT_BACKOFF_MULTIPLIER);
        assert!(policy.retry_on_rate_limit);
        assert!(policy.retry_on_timeout);
        assert!(policy.retry_on_server_error);
    }

    #[test]
    fn test_calculate_backoff() {
        let policy = RetryPolicy::new();

        // Attempt 0: 1000ms
        assert_eq!(policy.calculate_backoff(0), Duration::from_millis(1000));

        // Attempt 1: 2000ms (1000 * 2^1)
        assert_eq!(policy.calculate_backoff(1), Duration::from_millis(2000));

        // Attempt 2: 4000ms (1000 * 2^2)
        assert_eq!(policy.calculate_backoff(2), Duration::from_millis(4000));

        // Attempt 3: 8000ms (1000 * 2^3)
        assert_eq!(policy.calculate_backoff(3), Duration::from_millis(8000));
    }

    #[test]
    fn test_calculate_backoff_max_limit() {
        let policy = RetryPolicy::new().with_max_backoff(Duration::from_secs(5));

        // Should cap at 5000ms
        assert_eq!(policy.calculate_backoff(10), Duration::from_millis(5000));
    }

    #[test]
    fn test_with_fallback_models() {
        let policy = RetryPolicy::new()
            .with_fallback_models(vec!["model-1".to_string(), "model-2".to_string()]);

        assert_eq!(policy.next_fallback_model(1), Some("model-1"));
        assert_eq!(policy.next_fallback_model(2), Some("model-2"));
        assert_eq!(policy.next_fallback_model(3), None);
    }

    #[test]
    fn test_should_retry_rate_limit() {
        let policy = RetryPolicy::new();
        let error = super::super::ProviderError::RateLimited {
            retry_after_secs: 60,
            retry_after: None,
        };

        assert!(policy.should_retry(&error, 0));
        assert!(policy.should_retry(&error, 1));
        assert!(policy.should_retry(&error, 2));
        assert!(!policy.should_retry(&error, 3)); // max_retries = 3
    }

    #[test]
    fn test_should_retry_server_error() {
        let policy = RetryPolicy::new();
        let error = super::super::ProviderError::Api {
            status: 500,
            message: "Internal Server Error".to_string(),
        };

        assert!(policy.should_retry(&error, 0));
        assert!(policy.should_retry(&error, 1));
    }

    #[test]
    fn test_should_not_retry_client_error() {
        let policy = RetryPolicy::new();
        let error = super::super::ProviderError::Api {
            status: 400,
            message: "Bad Request".to_string(),
        };

        assert!(!policy.should_retry(&error, 0));
    }

    #[tokio::test]
    async fn test_execute_with_retry_success() {
        let policy = RetryPolicy::new();
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

        let attempts_clone = attempts.clone();
        let result = execute_with_retry(&policy, |_| {
            let a = attempts_clone.clone();
            async move {
                let count = a.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if count < 2 {
                    Err("Temporary error")
                } else {
                    Ok("Success")
                }
            }
        })
        .await;

        assert_eq!(result, Ok("Success"));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_execute_with_retry_max_retries() {
        let policy = RetryPolicy::new().with_max_retries(2);
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

        let attempts_clone = attempts.clone();
        let result = execute_with_retry(&policy, |_| {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err::<(), _>("Persistent error")
            }
        })
        .await;

        assert_eq!(result, Err("Persistent error"));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_execute_provider_with_retry_non_retryable() {
        // Non-retryable errors (e.g. InvalidRequest) should fail immediately.
        let policy = RetryPolicy::new();
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

        let attempts_clone = attempts.clone();
        let result: Result<(), _> = execute_provider_with_retry(&policy, || {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(super::super::ProviderError::InvalidRequest(
                    "bad input".into(),
                ))
            }
        })
        .await;

        assert!(result.is_err());
        // Should have been called exactly once (no retries).
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_execute_provider_with_retry_retryable_succeeds() {
        // Retryable error (Api 500) should be retried, then succeed.
        let policy = RetryPolicy::new()
            .with_initial_backoff(Duration::from_millis(1))
            .with_max_retries(3);
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

        let attempts_clone = attempts.clone();
        let result = execute_provider_with_retry(&policy, || {
            let a = attempts_clone.clone();
            async move {
                let count = a.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if count < 3 {
                    Err(super::super::ProviderError::Api {
                        status: 500,
                        message: "Internal Server Error".into(),
                    })
                } else {
                    Ok("success")
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_execute_provider_with_retry_uses_retry_after() {
        // When RateLimited has a retry_after, we should use it instead of the
        // default backoff. With max_retries=2, should_retry returns true for
        // attempts 0 and 1, so we get 3 total calls (initial + 2 retries).
        // The key assertion is that this completes quickly (uses 1ms retry_after
        // instead of the 60s default backoff).
        let policy = RetryPolicy::new()
            .with_initial_backoff(Duration::from_secs(60)) // very long default
            .with_max_retries(2);
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

        let attempts_clone = attempts.clone();
        let start = std::time::Instant::now();
        let result: Result<(), _> = execute_provider_with_retry(&policy, || {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(super::super::ProviderError::RateLimited {
                    retry_after_secs: 0,
                    retry_after: Some(Duration::from_millis(1)), // very short
                })
            }
        })
        .await;

        assert!(result.is_err());
        // initial + 2 retries = 3 total calls.
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
        // Must complete in <1s (proves we used 1ms retry_after, not 60s backoff).
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
