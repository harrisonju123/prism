use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// State of a single provider circuit.
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitState {
    /// Requests flow normally.
    Closed,
    /// Provider has failed too many times; requests are rejected.
    Open {
        opened_at: Instant,
        retry_at: Instant,
    },
    /// The cool-down period has elapsed; one probe request is allowed through.
    HalfOpen,
}

impl CircuitState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CircuitState::Closed => "closed",
            CircuitState::Open { .. } => "open",
            CircuitState::HalfOpen => "half_open",
        }
    }
}

struct Breaker {
    state: CircuitState,
    consecutive_failures: u32,
    last_failure_at: Option<Instant>,
}

impl Breaker {
    fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            last_failure_at: None,
        }
    }
}

/// Per-provider sliding-window circuit breaker.
///
/// Trips open after `failure_threshold` consecutive errors.
/// After `open_duration` the circuit moves to `HalfOpen` and allows one probe.
#[derive(Clone)]
pub struct CircuitBreaker {
    breakers: Arc<RwLock<HashMap<String, Breaker>>>,
    failure_threshold: u32,
    open_duration: Duration,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, open_duration_secs: u64) -> Self {
        Self {
            breakers: Arc::new(RwLock::new(HashMap::new())),
            failure_threshold,
            open_duration: Duration::from_secs(open_duration_secs),
        }
    }

    /// Returns `true` if the request is allowed through for `provider`.
    pub async fn is_allowed(&self, provider: &str) -> bool {
        let mut map = self.breakers.write().await;
        let b = map.entry(provider.to_string()).or_insert_with(Breaker::new);
        match &b.state {
            CircuitState::Closed => true,
            CircuitState::Open { retry_at, .. } => {
                if Instant::now() >= *retry_at {
                    b.state = CircuitState::HalfOpen;
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Call after a successful request for `provider`.
    pub async fn record_success(&self, provider: &str) {
        let mut map = self.breakers.write().await;
        if let Some(b) = map.get_mut(provider) {
            b.state = CircuitState::Closed;
            b.consecutive_failures = 0;
        }
    }

    /// Call after a failed request for `provider`.
    pub async fn record_failure(&self, provider: &str) {
        let mut map = self.breakers.write().await;
        let b = map.entry(provider.to_string()).or_insert_with(Breaker::new);
        b.consecutive_failures += 1;
        b.last_failure_at = Some(Instant::now());

        if b.consecutive_failures >= self.failure_threshold
            || b.state == CircuitState::HalfOpen
        {
            let now = Instant::now();
            b.state = CircuitState::Open {
                opened_at: now,
                retry_at: now + self.open_duration,
            };
            tracing::warn!(
                provider = provider,
                consecutive_failures = b.consecutive_failures,
                open_duration_secs = self.open_duration.as_secs(),
                "circuit breaker tripped open"
            );
        }
    }

    /// Snapshot of all circuit states for the health endpoint.
    pub async fn snapshot(&self) -> HashMap<String, &'static str> {
        let map = self.breakers.read().await;
        map.iter()
            .map(|(name, b)| (name.clone(), b.state.as_str()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_closed() {
        let cb = CircuitBreaker::new(3, 30);
        assert!(cb.is_allowed("anthropic").await);
    }

    #[tokio::test]
    async fn trips_open_after_threshold() {
        let cb = CircuitBreaker::new(2, 30);
        cb.record_failure("anthropic").await;
        assert!(cb.is_allowed("anthropic").await); // still closed
        cb.record_failure("anthropic").await;
        assert!(!cb.is_allowed("anthropic").await); // now open
    }

    #[tokio::test]
    async fn success_resets_failures() {
        let cb = CircuitBreaker::new(3, 30);
        cb.record_failure("openai").await;
        cb.record_failure("openai").await;
        cb.record_success("openai").await;
        cb.record_failure("openai").await;
        // Only 1 failure since reset — still closed
        assert!(cb.is_allowed("openai").await);
    }

    #[tokio::test]
    async fn snapshot_shows_provider_state() {
        let cb = CircuitBreaker::new(1, 30);
        cb.record_failure("anthropic").await;
        let snap = cb.snapshot().await;
        assert_eq!(snap.get("anthropic").copied(), Some("open"));
    }
}
