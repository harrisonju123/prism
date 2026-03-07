use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;

/// Lightweight Prometheus-compatible metrics collector using atomics and DashMap.
pub struct MetricsCollector {
    // Counters
    pub requests_total: AtomicU64,
    pub errors_total: AtomicU64,
    pub tokens_total: AtomicU64,
    pub cost_usd_millionths: AtomicU64, // stored as microdollars to avoid floats
    pub cache_hits_total: AtomicU64,
    pub rate_limited_total: AtomicU64,

    // Per-model counters
    pub requests_by_model: DashMap<String, AtomicU64>,
    pub errors_by_model: DashMap<String, AtomicU64>,

    // Histogram: request durations in milliseconds, stored as sorted buckets
    pub duration_samples: DashMap<String, Vec<u64>>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            requests_total: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            tokens_total: AtomicU64::new(0),
            cost_usd_millionths: AtomicU64::new(0),
            cache_hits_total: AtomicU64::new(0),
            rate_limited_total: AtomicU64::new(0),
            requests_by_model: DashMap::new(),
            errors_by_model: DashMap::new(),
            duration_samples: DashMap::new(),
        }
    }

    pub fn record_request(&self, model: &str, duration_ms: u64, is_error: bool) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        self.requests_by_model
            .entry(model.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);

        if is_error {
            self.errors_total.fetch_add(1, Ordering::Relaxed);
            self.errors_by_model
                .entry(model.to_string())
                .or_insert_with(|| AtomicU64::new(0))
                .fetch_add(1, Ordering::Relaxed);
        }

        // Record duration sample (keep last 1000 per model for quantile computation)
        let mut samples = self.duration_samples.entry(model.to_string()).or_default();
        if samples.len() >= 1000 {
            samples.remove(0);
        }
        samples.push(duration_ms);
    }

    pub fn record_tokens(&self, count: u64) {
        self.tokens_total.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_cost(&self, cost_usd: f64) {
        let microdollars = (cost_usd * 1_000_000.0) as u64;
        self.cost_usd_millionths
            .fetch_add(microdollars, Ordering::Relaxed);
    }

    pub fn record_cache_hit(&self) {
        self.cache_hits_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_rate_limited(&self) {
        self.rate_limited_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Render metrics in Prometheus text exposition format.
    pub fn render_prometheus(&self) -> String {
        let mut out = String::with_capacity(2048);

        // Counters
        out.push_str("# HELP prism_requests_total Total number of requests processed.\n");
        out.push_str("# TYPE prism_requests_total counter\n");
        push_counter(
            &mut out,
            "prism_requests_total",
            &[],
            self.requests_total.load(Ordering::Relaxed),
        );

        out.push_str("# HELP prism_errors_total Total number of errors.\n");
        out.push_str("# TYPE prism_errors_total counter\n");
        push_counter(
            &mut out,
            "prism_errors_total",
            &[],
            self.errors_total.load(Ordering::Relaxed),
        );

        out.push_str("# HELP prism_tokens_total Total tokens processed.\n");
        out.push_str("# TYPE prism_tokens_total counter\n");
        push_counter(
            &mut out,
            "prism_tokens_total",
            &[],
            self.tokens_total.load(Ordering::Relaxed),
        );

        let cost_micros = self.cost_usd_millionths.load(Ordering::Relaxed);
        let cost_usd = cost_micros as f64 / 1_000_000.0;
        out.push_str("# HELP prism_cost_usd_total Total estimated cost in USD.\n");
        out.push_str("# TYPE prism_cost_usd_total counter\n");
        push_counter_f64(&mut out, "prism_cost_usd_total", &[], cost_usd);

        out.push_str("# HELP prism_cache_hits_total Total cache hits.\n");
        out.push_str("# TYPE prism_cache_hits_total counter\n");
        push_counter(
            &mut out,
            "prism_cache_hits_total",
            &[],
            self.cache_hits_total.load(Ordering::Relaxed),
        );

        out.push_str("# HELP prism_rate_limited_total Total rate-limited requests.\n");
        out.push_str("# TYPE prism_rate_limited_total counter\n");
        push_counter(
            &mut out,
            "prism_rate_limited_total",
            &[],
            self.rate_limited_total.load(Ordering::Relaxed),
        );

        // Per-model request counts
        out.push_str("# HELP prism_model_requests_total Requests by model.\n");
        out.push_str("# TYPE prism_model_requests_total counter\n");
        for entry in self.requests_by_model.iter() {
            let model = entry.key();
            let count = entry.value().load(Ordering::Relaxed);
            push_counter(
                &mut out,
                "prism_model_requests_total",
                &[("model", model)],
                count,
            );
        }

        // Duration histogram (quantiles)
        out.push_str("# HELP prism_request_duration_seconds Request duration in seconds.\n");
        out.push_str("# TYPE prism_request_duration_seconds summary\n");
        for entry in self.duration_samples.iter() {
            let model = entry.key();
            let samples = entry.value();
            if samples.is_empty() {
                continue;
            }
            let mut sorted = samples.clone();
            sorted.sort_unstable();
            let count = sorted.len();
            let sum_ms: u64 = sorted.iter().sum();
            let sum_secs = sum_ms as f64 / 1000.0;

            for (quantile, label) in [(0.5, "0.5"), (0.95, "0.95"), (0.99, "0.99")] {
                let idx = ((count as f64 * quantile).ceil() as usize).min(count) - 1;
                let val_secs = sorted[idx] as f64 / 1000.0;
                out.push_str(&format!(
                    "prism_request_duration_seconds{{model=\"{model}\",quantile=\"{label}\"}} {val_secs:.6}\n"
                ));
            }
            out.push_str(&format!(
                "prism_request_duration_seconds_sum{{model=\"{model}\"}} {sum_secs:.6}\n"
            ));
            out.push_str(&format!(
                "prism_request_duration_seconds_count{{model=\"{model}\"}} {count}\n"
            ));
        }

        out
    }
}

fn push_counter(out: &mut String, name: &str, labels: &[(&str, &str)], value: u64) {
    if labels.is_empty() {
        out.push_str(&format!("{name} {value}\n"));
    } else {
        let label_str = labels
            .iter()
            .map(|(k, v)| format!("{k}=\"{v}\""))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&format!("{name}{{{label_str}}} {value}\n"));
    }
}

fn push_counter_f64(out: &mut String, name: &str, labels: &[(&str, &str)], value: f64) {
    if labels.is_empty() {
        out.push_str(&format!("{name} {value:.6}\n"));
    } else {
        let label_str = labels
            .iter()
            .map(|(k, v)| format!("{k}=\"{v}\""))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&format!("{name}{{{label_str}}} {value:.6}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn counter_increments() {
        let m = MetricsCollector::new();
        m.record_request("gpt-4o", 100, false);
        m.record_request("gpt-4o", 200, true);
        assert_eq!(m.requests_total.load(Ordering::Relaxed), 2);
        assert_eq!(m.errors_total.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn histogram_quantiles() {
        let m = MetricsCollector::new();
        for i in 1..=100 {
            m.record_request("gpt-4o", i * 10, false);
        }
        let text = m.render_prometheus();
        assert!(text.contains("prism_request_duration_seconds{model=\"gpt-4o\",quantile=\"0.5\"}"));
        assert!(
            text.contains("prism_request_duration_seconds{model=\"gpt-4o\",quantile=\"0.95\"}")
        );
        assert!(
            text.contains("prism_request_duration_seconds{model=\"gpt-4o\",quantile=\"0.99\"}")
        );
    }

    #[test]
    fn prometheus_text_format() {
        let m = MetricsCollector::new();
        m.record_request("test-model", 50, false);
        m.record_tokens(100);
        m.record_cost(0.005);
        m.record_cache_hit();
        m.record_rate_limited();

        let text = m.render_prometheus();
        assert!(text.contains("# TYPE prism_requests_total counter"));
        assert!(text.contains("prism_requests_total 1"));
        assert!(text.contains("prism_tokens_total 100"));
        assert!(text.contains("prism_cache_hits_total 1"));
        assert!(text.contains("prism_rate_limited_total 1"));
        assert!(text.contains("prism_cost_usd_total"));
    }

    #[test]
    fn concurrent_access() {
        let m = Arc::new(MetricsCollector::new());
        let mut handles = vec![];
        for _ in 0..10 {
            let m = m.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    m.record_request("model", 10, false);
                    m.record_tokens(5);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(m.requests_total.load(Ordering::Relaxed), 1000);
        assert_eq!(m.tokens_total.load(Ordering::Relaxed), 5000);
    }
}
