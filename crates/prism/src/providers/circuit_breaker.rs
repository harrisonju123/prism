use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
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
// Per-provider circuit breaker
// ---------------------------------------------------------------------------

pub struct ProviderCircuitBreaker {
    state: Mutex<CircuitState>,
    provider: String,
    failure_threshold: u32,
    open_duration: Duration,
    /// True while exactly one probe request is in-flight during HalfOpen.
    /// Only the winner of the compare_exchange gets through; all others see Open.
    probing: AtomicBool,
    /// When the current probe was claimed. Used to detect probes that timed out
    /// without calling record_success/record_failure (e.g. task cancelled/panicked).
    probe_started_at: Mutex<Option<Instant>>,
}

impl ProviderCircuitBreaker {
    pub fn new(provider: impl Into<String>, failure_threshold: u32, open_duration_secs: u64) -> Self {
        Self {
            state: Mutex::new(CircuitState::Closed {
                consecutive_failures: 0,
            }),
            provider: provider.into(),
            failure_threshold,
            open_duration: Duration::from_secs(open_duration_secs),
            probing: AtomicBool::new(false),
            probe_started_at: Mutex::new(None),
        }
    }

    /// Returns `Ok(())` if the circuit allows the request through.
    /// Returns `Err(retry_after_secs)` if it is Open or HalfOpen and already probing.
    pub fn check(&self) -> Result<(), u64> {
        let mut guard = self.state.lock().expect("circuit breaker lock poisoned");
        match &*guard {
            CircuitState::Closed { .. } => Ok(()),
            CircuitState::Open { opened_at } => {
                let elapsed = opened_at.elapsed();
                if elapsed >= self.open_duration {
                    tracing::info!(
                        provider = %self.provider,
                        "circuit breaker transitioning Open → HalfOpen"
                    );
                    *guard = CircuitState::HalfOpen;
                    // Only one probe at a time — claim the probe slot atomically.
                    // Reset a stale probe first (probe timed out without completing).
                    self.reset_stale_probe();
                    if self.probing.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                        *self.probe_started_at.lock().expect("probe_started_at lock poisoned") = Some(Instant::now());
                        Ok(())
                    } else {
                        // Another thread already has the probe; treat as still Open.
                        let remaining = self.open_duration.saturating_sub(elapsed).as_secs().max(1);
                        Err(remaining)
                    }
                } else {
                    let remaining = self.open_duration.saturating_sub(elapsed).as_secs().max(1);
                    Err(remaining)
                }
            }
            CircuitState::HalfOpen => {
                // A probe that never completed (timeout/panic) leaves probing=true forever.
                // Reset it if it has been in-flight longer than open_duration.
                self.reset_stale_probe();
                // Only one concurrent probe allowed.
                if self.probing.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    *self.probe_started_at.lock().expect("probe_started_at lock poisoned") = Some(Instant::now());
                    Ok(())
                } else {
                    Err(1)
                }
            }
        }
    }

    /// Reset `probing` if the current probe has been in-flight longer than `open_duration`,
    /// indicating the probe request was dropped without calling record_success/record_failure.
    /// Must be called while holding the state lock to avoid races with concurrent check() calls.
    fn reset_stale_probe(&self) {
        if self.probing.load(Ordering::Acquire) {
            let mut started = self.probe_started_at.lock().expect("probe_started_at lock poisoned");
            if let Some(t) = *started {
                if t.elapsed() >= self.open_duration {
                    tracing::warn!(
                        provider = %self.provider,
                        "circuit breaker probe timed out without completion, resetting"
                    );
                    self.probing.store(false, Ordering::Release);
                    *started = None;
                }
            }
        }
    }

    /// Call this after a successful provider response.
    pub fn record_success(&self) {
        let mut guard = self.state.lock().expect("circuit breaker lock poisoned");
        // Clear probe tracking before releasing the state lock to eliminate the
        // window where probing=false but state hasn't transitioned yet.
        self.probing.store(false, Ordering::Release);
        *self.probe_started_at.lock().expect("probe_started_at lock poisoned") = None;
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
        // Clear probe tracking before releasing the state lock to eliminate the
        // window where probing=false but state hasn't transitioned yet.
        self.probing.store(false, Ordering::Release);
        *self.probe_started_at.lock().expect("probe_started_at lock poisoned") = None;
        match &*guard {
            CircuitState::Closed {
                consecutive_failures,
            } => {
                let new_count = consecutive_failures + 1;
                if new_count >= self.failure_threshold {
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

/// Get or create the circuit breaker for `provider` using the supplied config values.
pub fn get_or_create(
    map: &CircuitBreakerMap,
    provider: &str,
    failure_threshold: u32,
    open_duration_secs: u64,
) -> Arc<ProviderCircuitBreaker> {
    map.entry(provider.to_string())
        .or_insert_with(|| {
            Arc::new(ProviderCircuitBreaker::new(provider, failure_threshold, open_duration_secs))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed() {
        let cb = ProviderCircuitBreaker::new("anthropic", 5, 60);
        assert!(cb.check().is_ok());
    }

    #[test]
    fn trips_open_after_threshold() {
        let cb = ProviderCircuitBreaker::new("anthropic", 3, 60);
        for _ in 0..3 {
            cb.record_failure();
        }
        // Now it should be open
        assert!(cb.check().is_err());
    }

    #[test]
    fn resets_on_success() {
        let cb = ProviderCircuitBreaker::new("openai", 3, 60);
        for _ in 0..2 {
            cb.record_failure();
        }
        cb.record_success();
        // One more failure should not trip
        cb.record_failure();
        assert!(cb.check().is_ok());
    }

    #[test]
    fn half_open_to_closed_on_success() {
        let cb = ProviderCircuitBreaker::new("openai", 5, 60);
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
        let cb = ProviderCircuitBreaker::new("openai", 5, 60);
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
