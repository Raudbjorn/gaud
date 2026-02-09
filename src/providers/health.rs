//! Circuit Breaker for Provider Health Tracking
//!
//! Implements the circuit breaker pattern to prevent cascading failures when
//! an LLM provider becomes unavailable. States transition as follows:
//!
//!   Closed (normal) --[N failures]--> Open (reject all)
//!   Open --[timeout expires]--> HalfOpen (allow probe)
//!   HalfOpen --[M successes]--> Closed
//!   HalfOpen --[any failure]--> Open

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Circuit State
// ---------------------------------------------------------------------------

/// State of a circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CircuitState {
    /// Normal operation -- requests are allowed through.
    #[default]
    Closed,
    /// Provider is failing -- all requests are rejected.
    Open,
    /// Testing recovery -- a limited number of probe requests are allowed.
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "closed"),
            Self::Open => write!(f, "open"),
            Self::HalfOpen => write!(f, "half-open"),
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration knobs for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Consecutive failures required to trip from Closed to Open.
    pub failure_threshold: u32,
    /// Consecutive successes in HalfOpen required to return to Closed.
    pub success_threshold: u32,
    /// How long to stay Open before moving to HalfOpen.
    pub timeout_duration: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            success_threshold: 2,
            timeout_duration: Duration::from_secs(30),
        }
    }
}

// ---------------------------------------------------------------------------
// CircuitBreaker
// ---------------------------------------------------------------------------

/// Per-provider circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    state: CircuitState,
    failure_count: u32,
    success_count: u32,
    last_failure: Option<Instant>,
    last_success: Option<Instant>,
    config: CircuitBreakerConfig,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::with_config(CircuitBreakerConfig::default())
    }
}

impl CircuitBreaker {
    /// Create a circuit breaker with default thresholds.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a circuit breaker with custom thresholds.
    pub fn with_config(config: CircuitBreakerConfig) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            last_failure: None,
            last_success: None,
            config,
        }
    }

    // -- queries -------------------------------------------------------------

    /// Current state.
    pub fn state(&self) -> CircuitState {
        self.state
    }

    /// Number of consecutive failures recorded.
    pub fn failure_count(&self) -> u32 {
        self.failure_count
    }

    /// Duration since the last failure, if any.
    pub fn time_since_failure(&self) -> Option<Duration> {
        self.last_failure.map(|t| t.elapsed())
    }

    /// Duration since the last success, if any.
    pub fn time_since_success(&self) -> Option<Duration> {
        self.last_success.map(|t| t.elapsed())
    }

    /// Whether the breaker currently allows a request through.
    ///
    /// Side-effect: if the breaker is Open and the timeout has elapsed it will
    /// transition to HalfOpen.
    pub fn can_execute(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(last) = self.last_failure {
                    if last.elapsed() >= self.config.timeout_duration {
                        self.state = CircuitState::HalfOpen;
                        self.success_count = 0;
                        return true;
                    }
                }
                false
            }
            CircuitState::HalfOpen => true,
        }
    }

    // -- recording -----------------------------------------------------------

    /// Record a successful request.
    pub fn record_success(&mut self) {
        self.failure_count = 0;
        self.last_success = Some(Instant::now());

        match self.state {
            CircuitState::HalfOpen => {
                self.success_count += 1;
                if self.success_count >= self.config.success_threshold {
                    self.state = CircuitState::Closed;
                    self.success_count = 0;
                }
            }
            _ => {
                self.state = CircuitState::Closed;
            }
        }
    }

    /// Record a failed request.
    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        self.success_count = 0;
        self.last_failure = Some(Instant::now());

        match self.state {
            CircuitState::HalfOpen => {
                // Any failure in half-open trips back to open.
                self.state = CircuitState::Open;
            }
            _ => {
                if self.failure_count >= self.config.failure_threshold {
                    self.state = CircuitState::Open;
                }
            }
        }
    }

    // -- manual control ------------------------------------------------------

    /// Reset to the Closed state (e.g. after admin intervention).
    pub fn reset(&mut self) {
        self.state = CircuitState::Closed;
        self.failure_count = 0;
        self.success_count = 0;
        self.last_failure = None;
    }

    /// Force the circuit Open (e.g. for maintenance).
    pub fn force_open(&mut self) {
        self.state = CircuitState::Open;
        self.last_failure = Some(Instant::now());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_state_is_closed() {
        let mut cb = CircuitBreaker::new();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.can_execute());
    }

    #[test]
    fn test_opens_after_threshold_failures() {
        let mut cb = CircuitBreaker::new(); // threshold = 3
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.can_execute());
    }

    #[test]
    fn test_success_resets_failure_count() {
        let mut cb = CircuitBreaker::new();
        cb.record_failure();
        cb.record_failure();
        cb.record_success();

        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count(), 0);
    }

    #[test]
    fn test_half_open_closes_after_successes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            timeout_duration: Duration::from_millis(0), // instant transition
        };
        let mut cb = CircuitBreaker::with_config(config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Timeout elapsed (0ms), should transition to HalfOpen.
        assert!(cb.can_execute());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen); // need 2

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_failure_reopens() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            timeout_duration: Duration::from_millis(0),
        };
        let mut cb = CircuitBreaker::with_config(config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        assert!(cb.can_execute()); // transitions to HalfOpen
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_manual_reset() {
        let mut cb = CircuitBreaker::new();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.can_execute());
        assert_eq!(cb.failure_count(), 0);
    }

    #[test]
    fn test_force_open() {
        let mut cb = CircuitBreaker::new();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.force_open();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.can_execute()); // timeout hasn't elapsed yet
    }

    #[test]
    fn test_circuit_state_display() {
        assert_eq!(CircuitState::Closed.to_string(), "closed");
        assert_eq!(CircuitState::Open.to_string(), "open");
        assert_eq!(CircuitState::HalfOpen.to_string(), "half-open");
    }

    #[test]
    fn test_time_since_failure() {
        let mut cb = CircuitBreaker::new();
        assert!(cb.time_since_failure().is_none());

        cb.record_failure();
        let elapsed = cb.time_since_failure().unwrap();
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn test_time_since_success() {
        let mut cb = CircuitBreaker::new();
        assert!(cb.time_since_success().is_none());

        cb.record_success();
        let elapsed = cb.time_since_success().unwrap();
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn test_open_blocks_until_timeout() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            timeout_duration: Duration::from_secs(60), // long timeout
        };
        let mut cb = CircuitBreaker::with_config(config);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.can_execute()); // 60s hasn't passed
    }
}
