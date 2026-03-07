use std::future::Future;

use rand::Rng;

use crate::config::RetryConfig;
use crate::error::Result;
use crate::types::ProviderResponse;

pub async fn with_retry<F, Fut>(config: &RetryConfig, operation: F) -> Result<ProviderResponse>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<ProviderResponse>>,
{
    let mut last_error;

    match operation().await {
        Ok(response) => return Ok(response),
        Err(e) => {
            if !e.is_retryable() || config.max_retries == 0 {
                return Err(e);
            }
            tracing::warn!(
                error = %e,
                attempt = 1,
                max_retries = config.max_retries,
                "retryable error, will retry"
            );
            last_error = e;
        }
    }

    for attempt in 1..=config.max_retries {
        let delay_ms = compute_delay(config, attempt);
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

        match operation().await {
            Ok(response) => {
                tracing::info!(attempt = attempt + 1, "retry succeeded");
                return Ok(response);
            }
            Err(e) => {
                if !e.is_retryable() {
                    return Err(e);
                }
                tracing::warn!(
                    error = %e,
                    attempt = attempt + 1,
                    max_retries = config.max_retries,
                    delay_ms,
                    "retry failed"
                );
                last_error = e;
            }
        }
    }

    Err(last_error)
}

fn compute_delay(config: &RetryConfig, attempt: u32) -> u64 {
    let exp_delay = config
        .base_delay_ms
        .saturating_mul(2u64.saturating_pow(attempt - 1));
    let capped = exp_delay.min(config.max_delay_ms);

    if config.jitter {
        let jitter: u64 = rand::rng().random_range(0..=capped / 2);
        capped.saturating_add(jitter).min(config.max_delay_ms)
    } else {
        capped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PrismError;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn test_config(max_retries: u32) -> RetryConfig {
        RetryConfig {
            max_retries,
            base_delay_ms: 1,
            max_delay_ms: 10,
            jitter: false,
        }
    }

    fn success_response() -> ProviderResponse {
        ProviderResponse::Complete(crate::types::ChatCompletionResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![],
            usage: None,
            extra: serde_json::Map::new(),
        })
    }

    #[tokio::test]
    async fn succeeds_on_first_try() {
        let config = test_config(3);
        let call_count = AtomicU32::new(0);

        let result = with_retry(&config, || {
            call_count.fetch_add(1, Ordering::SeqCst);
            async { Ok(success_response()) }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_on_retryable_then_succeeds() {
        let config = test_config(3);
        let call_count = AtomicU32::new(0);

        let result = with_retry(&config, || {
            let n = call_count.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(PrismError::Timeout(5000))
                } else {
                    Ok(success_response())
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn no_retry_on_non_retryable() {
        let config = test_config(3);
        let call_count = AtomicU32::new(0);

        let result = with_retry(&config, || {
            call_count.fetch_add(1, Ordering::SeqCst);
            async { Err(PrismError::BadRequest("bad input".into())) }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn exhausts_retries() {
        let config = test_config(2);
        let call_count = AtomicU32::new(0);

        let result = with_retry(&config, || {
            call_count.fetch_add(1, Ordering::SeqCst);
            async { Err(PrismError::Timeout(5000)) }
        })
        .await;

        assert!(result.is_err());
        // 1 initial + 2 retries = 3 total
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn no_retry_when_max_retries_zero() {
        let config = test_config(0);
        let call_count = AtomicU32::new(0);

        let result = with_retry(&config, || {
            call_count.fetch_add(1, Ordering::SeqCst);
            async { Err(PrismError::Timeout(5000)) }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn compute_delay_exponential_backoff() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay_ms: 500,
            max_delay_ms: 10_000,
            jitter: false,
        };

        assert_eq!(compute_delay(&config, 1), 500); // 500 * 2^0
        assert_eq!(compute_delay(&config, 2), 1000); // 500 * 2^1
        assert_eq!(compute_delay(&config, 3), 2000); // 500 * 2^2
        assert_eq!(compute_delay(&config, 4), 4000); // 500 * 2^3
        assert_eq!(compute_delay(&config, 5), 8000); // 500 * 2^4
    }

    #[test]
    fn compute_delay_caps_at_max() {
        let config = RetryConfig {
            max_retries: 10,
            base_delay_ms: 500,
            max_delay_ms: 5000,
            jitter: false,
        };

        assert_eq!(compute_delay(&config, 5), 5000); // 500 * 16 = 8000, capped at 5000
    }

    #[test]
    fn compute_delay_with_jitter_bounded() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 100,
            max_delay_ms: 10_000,
            jitter: true,
        };

        for _ in 0..100 {
            let delay = compute_delay(&config, 1);
            // base=100, jitter adds 0..50, so delay in [100, 150]
            assert!(delay >= 100 && delay <= 150, "delay was {delay}");
        }
    }

    #[tokio::test]
    async fn stops_retrying_on_non_retryable_mid_sequence() {
        let config = test_config(3);
        let call_count = AtomicU32::new(0);

        let result = with_retry(&config, || {
            let n = call_count.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Err(PrismError::Timeout(5000))
                } else {
                    Err(PrismError::Unauthorized)
                }
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn is_retryable_provider_errors() {
        assert!(PrismError::Provider("status 500".into()).is_retryable());
        assert!(PrismError::Provider("502 Bad Gateway".into()).is_retryable());
        assert!(PrismError::Provider("503 Service Unavailable".into()).is_retryable());
        assert!(PrismError::Provider("504 Gateway Timeout".into()).is_retryable());
        assert!(!PrismError::Provider("400 Bad Request".into()).is_retryable());
        assert!(!PrismError::Provider("401 Unauthorized".into()).is_retryable());
    }

    #[test]
    fn is_retryable_other_errors() {
        assert!(PrismError::Timeout(5000).is_retryable());
        assert!(!PrismError::BadRequest("bad".into()).is_retryable());
        assert!(!PrismError::Unauthorized.is_retryable());
        assert!(
            !PrismError::RateLimited {
                retry_after_secs: None
            }
            .is_retryable()
        );
        assert!(!PrismError::BudgetExceeded.is_retryable());
        assert!(!PrismError::Internal("oops".into()).is_retryable());
    }
}
