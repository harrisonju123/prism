use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::Mutex;
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
    pub async fn check(&self) -> Result<(), u64> {
        let mut guard = self.state.lock().await;
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
    pub async fn record_success(&self) {
        let mut guard = self.state.lock().await;
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
    pub async fn record_failure(&self) {
        let mut guard = self.state.lock().await;
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
    pub async fn state_name(&self) -> &'static str {
        self.state.lock().await.name()
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
    if let Some(cb) = map.get(provider) {
        return cb.clone();
    }
    let cb = Arc::new(ProviderCircuitBreaker::new(provider));
    map.insert(provider.to_string(), cb.clone());
    cb
}

// ---------------------------------------------------------------------------
// Helpers — classify a PrismError as a provider 5xx
// ---------------------------------------------------------------------------

/// Returns `true` if the error should trip the circuit breaker (provider 5xx).
pub fn is_provider_5xx(err: &crate::error::PrismError) -> bool {
    match err {
        crate::error::PrismError::Provider(msg) => {
            msg.contains("500")
                || msg.contains("502")
                || msg.contains("503")
                || msg.contains("504")
                || msg.contains("529")
        }
        crate::error::PrismError::Timeout(_) => false,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_closed() {
        let cb = ProviderCircuitBreaker::new("anthropic");
        assert!(cb.check().await.is_ok());
    }

    #[tokio::test]
    async fn trips_open_after_threshold() {
        let cb = ProviderCircuitBreaker::new("anthropic");
        for _ in 0..FAILURE_THRESHOLD {
            cb.record_failure().await;
        }
        // Now it should be open
        assert!(cb.check().await.is_err());
    }

    #[tokio::test]
    async fn resets_on_success() {
        let cb = ProviderCircuitBreaker::new("openai");
        for _ in 0..(FAILURE_THRESHOLD - 1) {
            cb.record_failure().await;
        }
        cb.record_success().await;
        // One more failure should not trip
        cb.record_failure().await;
        assert!(cb.check().await.is_ok());
    }

    #[tokio::test]
    async fn half_open_to_closed_on_success() {
        let cb = ProviderCircuitBreaker::new("openai");
        // Force into HalfOpen by manipulating state directly
        {
            let mut guard = cb.state.lock().await;
            *guard = CircuitState::HalfOpen;
        }
        cb.record_success().await;
        assert_eq!(cb.state_name().await, "closed");
    }

    #[tokio::test]
    async fn half_open_to_open_on_failure() {
        let cb = ProviderCircuitBreaker::new("openai");
        {
            let mut guard = cb.state.lock().await;
            *guard = CircuitState::HalfOpen;
        }
        cb.record_failure().await;
        assert_eq!(cb.state_name().await, "open");
    }

    #[test]
    fn is_provider_5xx_detects_server_errors() {
        use crate::error::PrismError;
        assert!(is_provider_5xx(&PrismError::Provider("HTTP 500".into())));
        assert!(is_provider_5xx(&PrismError::Provider(
            "503 service unavailable".into()
        )));
        assert!(!is_provider_5xx(&PrismError::Provider("HTTP 429".into())));
        assert!(!is_provider_5xx(&PrismError::Unauthorized));
    }
}
