use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use dashmap::DashMap;
use tokio::time::Instant;

// ---------------------------------------------------------------------------
// States
// ---------------------------------------------------------------------------

/// The state of a circuit breaker for a single provider.
#[derive(Debug, Clone)]
pub enum CircuitState {
    /// Normal operation — requests are forwarded.
    Closed { consecutive_failures: u32 },
    /// Provider is failing — requests are rejected immediately.
    Open { opened_at: Instant },
    /// One test request is allowed through to probe recovery.
    HalfOpen,
}

impl CircuitState {
    fn name(&self) -> &'static str {
        match self {
            CircuitState::Closed { .. } => "closed",
            CircuitState::Open { .. } => "open",
            CircuitState::HalfOpen => "half_open",
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// How many consecutive 5xx errors before tripping open.
const FAILURE_THRESHOLD: u32 = 5;
/// How long to wait in Open before transitioning to HalfOpen.
const BACKOFF: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Per-provider circuit breaker
// ---------------------------------------------------------------------------

pub struct ProviderCircuitBreaker {
    state: Mutex<CircuitState>,
    provider: String,
}

impl ProviderCircuitBreaker {
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            state: Mutex::new(CircuitState::Closed {
                consecutive_failures: 0,
            }),
            provider: provider.into(),
        }
    }

    /// Returns `Ok(())` if the circuit allows the request through.
    /// Returns `Err(retry_after_secs)` if it is Open.
    pub fn check(&self) -> Result<(), u64> {
        let mut guard = self.state.lock().expect("circuit breaker lock poisoned");
        match &*guard {
            CircuitState::Closed { .. } => Ok(()),
            CircuitState::Open { opened_at } => {
                let elapsed = opened_at.elapsed();
                if elapsed >= BACKOFF {
                    tracing::info!(
                        provider = %self.provider,
                        "circuit breaker transitioning Open → HalfOpen"
                    );
                    *guard = CircuitState::HalfOpen;
                    Ok(())
                } else {
                    let remaining = BACKOFF.saturating_sub(elapsed).as_secs().max(1);
                    Err(remaining)
                }
            }
            CircuitState::HalfOpen => Ok(()),
        }
    }

    /// Call this after a successful provider response.
    pub fn record_success(&self) {
        let mut guard = self.state.lock().expect("circuit breaker lock poisoned");
        match &*guard {
            CircuitState::HalfOpen => {
                tracing::info!(
                    provider = %self.provider,
                    "circuit breaker transitioning HalfOpen → Closed"
                );
                *guard = CircuitState::Closed {
                    consecutive_failures: 0,
                };
            }
            CircuitState::Closed { .. } => {
                *guard = CircuitState::Closed {
                    consecutive_failures: 0,
                };
            }
            CircuitState::Open { .. } => {
                // Shouldn't happen — success while open means probe let through
                *guard = CircuitState::Closed {
                    consecutive_failures: 0,
                };
            }
        }
    }

    /// Call this after a 5xx provider error. Trips open after threshold.
    pub fn record_failure(&self) {
        let mut guard = self.state.lock().expect("circuit breaker lock poisoned");
        match &*guard {
            CircuitState::Closed {
                consecutive_failures,
            } => {
                let new_count = consecutive_failures + 1;
                if new_count >= FAILURE_THRESHOLD {
                    tracing::warn!(
                        provider = %self.provider,
                        failures = new_count,
                        "circuit breaker tripped: Closed → Open"
                    );
                    *guard = CircuitState::Open {
                        opened_at: Instant::now(),
                    };
                } else {
                    *guard = CircuitState::Closed {
                        consecutive_failures: new_count,
                    };
                }
            }
            CircuitState::HalfOpen => {
                tracing::warn!(
                    provider = %self.provider,
                    "circuit breaker probe failed: HalfOpen → Open"
                );
                *guard = CircuitState::Open {
                    opened_at: Instant::now(),
                };
            }
            CircuitState::Open { .. } => {} // already open, leave as is
        }
    }

    /// Return the state name as a string for health reporting.
    pub fn state_name(&self) -> &'static str {
        self.state.lock().expect("circuit breaker lock poisoned").name()
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Shared map from provider name → circuit breaker, stored on `AppState`.
pub type CircuitBreakerMap = Arc<DashMap<String, Arc<ProviderCircuitBreaker>>>;

/// Create a new empty circuit breaker map.
pub fn new_circuit_breaker_map() -> CircuitBreakerMap {
    Arc::new(DashMap::new())
}

/// Get or create the circuit breaker for `provider`.
pub fn get_or_create(map: &CircuitBreakerMap, provider: &str) -> Arc<ProviderCircuitBreaker> {
    map.entry(provider.to_string())
        .or_insert_with(|| Arc::new(ProviderCircuitBreaker::new(provider)))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed() {
        let cb = ProviderCircuitBreaker::new("anthropic");
        assert!(cb.check().is_ok());
    }

    #[test]
    fn trips_open_after_threshold() {
        let cb = ProviderCircuitBreaker::new("anthropic");
        for _ in 0..FAILURE_THRESHOLD {
            cb.record_failure();
        }
        // Now it should be open
        assert!(cb.check().is_err());
    }

    #[test]
    fn resets_on_success() {
        let cb = ProviderCircuitBreaker::new("openai");
        for _ in 0..(FAILURE_THRESHOLD - 1) {
            cb.record_failure();
        }
        cb.record_success();
        // One more failure should not trip
        cb.record_failure();
        assert!(cb.check().is_ok());
    }

    #[test]
    fn half_open_to_closed_on_success() {
        let cb = ProviderCircuitBreaker::new("openai");
        // Force into HalfOpen by manipulating state directly
        {
            let mut guard = cb.state.lock().expect("circuit breaker lock poisoned");
            *guard = CircuitState::HalfOpen;
        }
        cb.record_success();
        assert_eq!(cb.state_name(), "closed");
    }

    #[test]
    fn half_open_to_open_on_failure() {
        let cb = ProviderCircuitBreaker::new("openai");
        {
            let mut guard = cb.state.lock().expect("circuit breaker lock poisoned");
            *guard = CircuitState::HalfOpen;
        }
        cb.record_failure();
        assert_eq!(cb.state_name(), "open");
    }

    #[test]
    fn is_provider_server_error_detects_server_errors() {
        use crate::error::PrismError;
        assert!(PrismError::Provider("HTTP 500".into()).is_provider_server_error());
        assert!(PrismError::Provider("503 service unavailable".into()).is_provider_server_error());
        assert!(!PrismError::Provider("HTTP 429".into()).is_provider_server_error());
        assert!(!PrismError::Unauthorized.is_provider_server_error());
    }
}
