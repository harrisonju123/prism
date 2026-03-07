use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

/// Load balancing strategy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BalancerStrategy {
    RoundRobin,
    LeastBusy,
    LatencyBased,
    UsageBased,
}

impl Default for BalancerStrategy {
    fn default() -> Self {
        Self::RoundRobin
    }
}

/// Tracks in-flight requests and latency per provider for load balancing.
pub struct LoadBalancer {
    /// In-flight request counts per provider.
    in_flight: DashMap<String, AtomicU64>,
    /// Rolling latency samples per provider (last N requests).
    latencies: DashMap<String, LatencyTracker>,
    /// Round-robin counter.
    rr_counter: AtomicU64,
}

struct LatencyTracker {
    samples: VecDeque<u32>,
    max_samples: usize,
}

impl LatencyTracker {
    fn new(max_samples: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(max_samples),
            max_samples,
        }
    }

    fn record(&mut self, latency_ms: u32) {
        if self.samples.len() >= self.max_samples {
            self.samples.pop_front();
        }
        self.samples.push_back(latency_ms);
    }

    fn p50(&self) -> f64 {
        if self.samples.is_empty() {
            return f64::MAX;
        }
        let mut sorted: Vec<u32> = self.samples.iter().copied().collect();
        sorted.sort_unstable();
        let idx = sorted.len() / 2;
        sorted[idx] as f64
    }
}

impl LoadBalancer {
    pub fn new() -> Self {
        Self {
            in_flight: DashMap::new(),
            latencies: DashMap::new(),
            rr_counter: AtomicU64::new(0),
        }
    }

    /// Select the best provider from candidates using the given strategy.
    pub fn select(
        &self,
        candidates: &[String],
        strategy: &BalancerStrategy,
        budget_headroom: Option<&DashMap<String, f64>>,
    ) -> Option<String> {
        if candidates.is_empty() {
            return None;
        }
        if candidates.len() == 1 {
            return Some(candidates[0].clone());
        }

        match strategy {
            BalancerStrategy::RoundRobin => {
                let idx =
                    self.rr_counter.fetch_add(1, Ordering::Relaxed) as usize % candidates.len();
                Some(candidates[idx].clone())
            }
            BalancerStrategy::LeastBusy => {
                let mut best = &candidates[0];
                let mut best_count = self.get_in_flight(&candidates[0]);
                for c in &candidates[1..] {
                    let count = self.get_in_flight(c);
                    if count < best_count {
                        best = c;
                        best_count = count;
                    }
                }
                Some(best.clone())
            }
            BalancerStrategy::LatencyBased => {
                let mut best = &candidates[0];
                let mut best_p50 = self.get_p50(&candidates[0]);
                for c in &candidates[1..] {
                    let p50 = self.get_p50(c);
                    if p50 < best_p50 {
                        best = c;
                        best_p50 = p50;
                    }
                }
                Some(best.clone())
            }
            BalancerStrategy::UsageBased => {
                // Pick provider with most budget headroom
                let headroom = match budget_headroom {
                    Some(h) => h,
                    None => return Some(candidates[0].clone()),
                };
                let mut best = &candidates[0];
                let mut best_headroom =
                    headroom.get(&candidates[0]).map(|v| *v).unwrap_or(f64::MAX);
                for c in &candidates[1..] {
                    let h = headroom.get(c).map(|v| *v).unwrap_or(f64::MAX);
                    if h > best_headroom {
                        best = c;
                        best_headroom = h;
                    }
                }
                Some(best.clone())
            }
        }
    }

    /// Record start of a request (increment in-flight counter).
    pub fn request_start(&self, provider: &str) {
        self.in_flight
            .entry(provider.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record end of a request (decrement in-flight counter, record latency).
    pub fn request_end(&self, provider: &str, latency_ms: u32) {
        if let Some(counter) = self.in_flight.get(provider) {
            let val = counter.load(Ordering::Relaxed);
            if val > 0 {
                counter.fetch_sub(1, Ordering::Relaxed);
            }
        }
        self.latencies
            .entry(provider.to_string())
            .or_insert_with(|| LatencyTracker::new(100))
            .record(latency_ms);
    }

    fn get_in_flight(&self, provider: &str) -> u64 {
        self.in_flight
            .get(provider)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    fn get_p50(&self, provider: &str) -> f64 {
        self.latencies
            .get(provider)
            .map(|t| t.p50())
            .unwrap_or(f64::MAX)
    }
}

/// Determine if a status code is retryable.
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_cycles() {
        let lb = LoadBalancer::new();
        let candidates = vec!["a".into(), "b".into(), "c".into()];
        let first = lb
            .select(&candidates, &BalancerStrategy::RoundRobin, None)
            .unwrap();
        let second = lb
            .select(&candidates, &BalancerStrategy::RoundRobin, None)
            .unwrap();
        let third = lb
            .select(&candidates, &BalancerStrategy::RoundRobin, None)
            .unwrap();
        let fourth = lb
            .select(&candidates, &BalancerStrategy::RoundRobin, None)
            .unwrap();
        assert_eq!(first, "a");
        assert_eq!(second, "b");
        assert_eq!(third, "c");
        assert_eq!(fourth, "a");
    }

    #[test]
    fn least_busy_picks_idle() {
        let lb = LoadBalancer::new();
        lb.request_start("a");
        lb.request_start("a");
        lb.request_start("b");
        // c has 0 in-flight
        let candidates = vec!["a".into(), "b".into(), "c".into()];
        let pick = lb
            .select(&candidates, &BalancerStrategy::LeastBusy, None)
            .unwrap();
        assert_eq!(pick, "c");
    }

    #[test]
    fn latency_based_picks_fastest() {
        let lb = LoadBalancer::new();
        lb.request_end("a", 200);
        lb.request_end("b", 50);
        lb.request_end("c", 150);
        let candidates = vec!["a".into(), "b".into(), "c".into()];
        let pick = lb
            .select(&candidates, &BalancerStrategy::LatencyBased, None)
            .unwrap();
        assert_eq!(pick, "b");
    }

    #[test]
    fn usage_based_picks_most_headroom() {
        let lb = LoadBalancer::new();
        let headroom = DashMap::new();
        headroom.insert("a".to_string(), 10.0);
        headroom.insert("b".to_string(), 50.0);
        headroom.insert("c".to_string(), 25.0);
        let candidates = vec!["a".into(), "b".into(), "c".into()];
        let pick = lb
            .select(&candidates, &BalancerStrategy::UsageBased, Some(&headroom))
            .unwrap();
        assert_eq!(pick, "b");
    }

    #[test]
    fn in_flight_tracking() {
        let lb = LoadBalancer::new();
        lb.request_start("a");
        lb.request_start("a");
        assert_eq!(lb.get_in_flight("a"), 2);
        lb.request_end("a", 100);
        assert_eq!(lb.get_in_flight("a"), 1);
        lb.request_end("a", 150);
        assert_eq!(lb.get_in_flight("a"), 0);
    }

    #[test]
    fn retryable_status_codes() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(502));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(504));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(500));
    }

    #[test]
    fn single_candidate_always_selected() {
        let lb = LoadBalancer::new();
        let candidates = vec!["only".into()];
        for strategy in [
            BalancerStrategy::RoundRobin,
            BalancerStrategy::LeastBusy,
            BalancerStrategy::LatencyBased,
            BalancerStrategy::UsageBased,
        ] {
            let pick = lb.select(&candidates, &strategy, None).unwrap();
            assert_eq!(pick, "only");
        }
    }
}
