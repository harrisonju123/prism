use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::providers::ProviderRegistry;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderHealth {
    pub provider: String,
    pub status: HealthStatus,
    pub last_check: Option<DateTime<Utc>>,
    pub consecutive_failures: u32,
    pub error: Option<String>,
}

pub struct ProviderHealthTracker {
    entries: DashMap<String, ProviderHealth>,
    failure_threshold: u32,
}

impl ProviderHealthTracker {
    pub fn new(failure_threshold: u32) -> Self {
        Self {
            entries: DashMap::new(),
            failure_threshold,
        }
    }

    pub fn record_success(&self, provider: &str) {
        let mut entry =
            self.entries
                .entry(provider.to_string())
                .or_insert_with(|| ProviderHealth {
                    provider: provider.to_string(),
                    status: HealthStatus::Unknown,
                    last_check: None,
                    consecutive_failures: 0,
                    error: None,
                });
        entry.status = HealthStatus::Healthy;
        entry.consecutive_failures = 0;
        entry.last_check = Some(Utc::now());
        entry.error = None;
    }

    pub fn record_failure(&self, provider: &str, error: String) -> HealthStatus {
        let mut entry =
            self.entries
                .entry(provider.to_string())
                .or_insert_with(|| ProviderHealth {
                    provider: provider.to_string(),
                    status: HealthStatus::Unknown,
                    last_check: None,
                    consecutive_failures: 0,
                    error: None,
                });
        entry.consecutive_failures += 1;
        entry.last_check = Some(Utc::now());
        entry.error = Some(error);
        if entry.consecutive_failures >= self.failure_threshold {
            entry.status = HealthStatus::Degraded;
        }
        entry.status.clone()
    }

    pub fn is_available(&self, provider: &str) -> bool {
        self.entries
            .get(provider)
            .map(|e| e.status != HealthStatus::Degraded)
            .unwrap_or(true)
    }

    pub fn snapshot(&self) -> Vec<ProviderHealth> {
        self.entries.iter().map(|e| e.clone()).collect()
    }
}

static PROVIDER_HEALTH_URLS: &[(&str, &str)] = &[
    ("openai", "https://api.openai.com"),
    ("anthropic", "https://api.anthropic.com"),
    ("google", "https://generativelanguage.googleapis.com"),
    ("groq", "https://api.groq.com"),
    ("mistral", "https://api.mistral.ai"),
    ("deepseek", "https://api.deepseek.com"),
    ("together", "https://api.together.xyz"),
    ("azure", "https://management.azure.com"),
    ("azure_openai", "https://management.azure.com"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tracker_empty_snapshot() {
        let t = ProviderHealthTracker::new(3);
        assert!(t.snapshot().is_empty());
    }

    #[test]
    fn record_success_sets_healthy() {
        let t = ProviderHealthTracker::new(3);
        t.record_success("openai");
        let snap = t.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].status, HealthStatus::Healthy);
        assert_eq!(snap[0].consecutive_failures, 0);
    }

    #[test]
    fn record_failure_increments() {
        let t = ProviderHealthTracker::new(3);
        t.record_failure("openai", "timeout".into());
        let snap = t.snapshot();
        assert_eq!(snap[0].consecutive_failures, 1);
        // 1 failure < threshold=3, should NOT be Degraded yet
        assert_ne!(snap[0].status, HealthStatus::Degraded);
    }

    #[test]
    fn failure_threshold_triggers_degraded() {
        let t = ProviderHealthTracker::new(3);
        t.record_failure("openai", "e".into());
        t.record_failure("openai", "e".into());
        t.record_failure("openai", "e".into());
        let snap = t.snapshot();
        assert_eq!(snap[0].status, HealthStatus::Degraded);
    }

    #[test]
    fn is_available_unknown_returns_true() {
        let t = ProviderHealthTracker::new(3);
        assert!(t.is_available("anthropic"));
    }

    #[test]
    fn is_available_degraded_returns_false() {
        let t = ProviderHealthTracker::new(1);
        t.record_failure("groq", "err".into());
        assert!(!t.is_available("groq"));
    }

    #[test]
    fn success_resets_failure_counter() {
        let t = ProviderHealthTracker::new(5);
        t.record_failure("mistral", "e".into());
        t.record_failure("mistral", "e".into());
        t.record_success("mistral");
        let snap = t.snapshot();
        assert_eq!(snap[0].status, HealthStatus::Healthy);
        assert_eq!(snap[0].consecutive_failures, 0);
    }

    #[tokio::test]
    async fn health_check_records_success() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock_server)
            .await;

        let tracker = ProviderHealthTracker::new(3);
        let urls = vec![("test-provider".to_string(), mock_server.uri())];
        let client = reqwest::Client::new();

        check_providers_once(&tracker, &urls, &client).await;

        let snap = tracker.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].status, HealthStatus::Healthy);
        assert_eq!(snap[0].consecutive_failures, 0);
    }

    #[tokio::test]
    async fn health_check_records_failure_on_connection_error() {
        let tracker = ProviderHealthTracker::new(3);
        let urls = vec![(
            "dead-provider".to_string(),
            "http://127.0.0.1:1".to_string(),
        )];
        let client = reqwest::Client::new();

        check_providers_once(&tracker, &urls, &client).await;

        let snap = tracker.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].consecutive_failures, 1);
        assert!(snap[0].error.is_some());
    }
}

pub(crate) async fn check_providers_once(
    tracker: &ProviderHealthTracker,
    urls: &[(String, String)],
    http_client: &reqwest::Client,
) {
    for (provider, url) in urls {
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            http_client.head(url.as_str()).send(),
        )
        .await;
        match result {
            Ok(Ok(_)) => {
                tracker.record_success(provider);
                tracing::debug!(provider = provider, "health check ok");
            }
            Ok(Err(e)) => {
                let status = tracker.record_failure(provider, e.to_string());
                tracing::warn!(
                    provider = provider,
                    error = %e,
                    status = ?status,
                    "health check failed"
                );
            }
            Err(_) => {
                let status = tracker.record_failure(provider, "timeout".into());
                tracing::warn!(
                    provider = provider,
                    status = ?status,
                    "health check timed out"
                );
            }
        }
    }
}

pub async fn spawn_health_checker(
    tracker: Arc<ProviderHealthTracker>,
    registry: Arc<ProviderRegistry>,
    http_client: reqwest::Client,
    interval_secs: u64,
    cancel: CancellationToken,
) {
    let configured: Vec<String> = registry.list().iter().map(|s| s.to_string()).collect();
    let urls: Vec<(String, String)> = PROVIDER_HEALTH_URLS
        .iter()
        .filter(|(name, _)| configured.iter().any(|c| c == name))
        .map(|(name, url)| (name.to_string(), url.to_string()))
        .collect();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {
                    check_providers_once(&tracker, &urls, &http_client).await;
                }
                _ = cancel.cancelled() => break,
            }
        }
    });
}
