use std::future::Future;
use std::time::Duration;

use rand::Rng;

use crate::Result;

pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            base_delay_ms: 1000,
            max_delay_ms: 30_000,
            jitter: true,
        }
    }
}

impl RetryConfig {
    pub fn with_max_retries(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Self::default()
        }
    }
}

pub async fn with_retry<F, Fut, T>(config: &RetryConfig, operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut last_error;

    match operation().await {
        Ok(val) => return Ok(val),
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
        let retry_after = last_error.retry_after_secs();
        let computed = compute_delay(config, attempt);
        let delay_ms = match retry_after {
            Some(secs) => computed.max(secs * 1000),
            None => computed,
        };
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        match operation().await {
            Ok(val) => {
                tracing::info!(attempt = attempt + 1, "retry succeeded");
                return Ok(val);
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
        let jitter: u64 = rand::rng().random_range(0..=(capped / 2).max(1));
        capped.saturating_add(jitter).min(config.max_delay_ms)
    } else {
        capped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientError;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn fast_config(max_retries: u32) -> RetryConfig {
        RetryConfig {
            max_retries,
            base_delay_ms: 1,
            max_delay_ms: 10,
            jitter: false,
        }
    }

    fn retryable_err() -> ClientError {
        ClientError::Api {
            status: 503,
            message: "service unavailable".into(),
            retry_after_secs: None,
        }
    }

    fn non_retryable_err() -> ClientError {
        ClientError::Api {
            status: 400,
            message: "bad request".into(),
            retry_after_secs: None,
        }
    }

    #[tokio::test]
    async fn succeeds_on_first_try() {
        let config = fast_config(3);
        let count = AtomicU32::new(0);
        let result: Result<u32> = with_retry(&config, || {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(42) }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let config = fast_config(3);
        let count = AtomicU32::new(0);
        let result: Result<u32> = with_retry(&config, || {
            let n = count.fetch_add(1, Ordering::SeqCst);
            async move { if n < 2 { Err(retryable_err()) } else { Ok(42) } }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn no_retry_on_non_retryable() {
        let config = fast_config(3);
        let count = AtomicU32::new(0);
        let result: Result<u32> = with_retry(&config, || {
            count.fetch_add(1, Ordering::SeqCst);
            async { Err(non_retryable_err()) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn exhausts_retries() {
        let config = fast_config(2);
        let count = AtomicU32::new(0);
        let result: Result<u32> = with_retry(&config, || {
            count.fetch_add(1, Ordering::SeqCst);
            async { Err(retryable_err()) }
        })
        .await;
        assert!(result.is_err());
        // 1 initial + 2 retries = 3
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn no_retry_when_max_zero() {
        let config = fast_config(0);
        let count = AtomicU32::new(0);
        let result: Result<u32> = with_retry(&config, || {
            count.fetch_add(1, Ordering::SeqCst);
            async { Err(retryable_err()) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn compute_delay_exponential() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay_ms: 500,
            max_delay_ms: 10_000,
            jitter: false,
        };
        assert_eq!(compute_delay(&config, 1), 500);
        assert_eq!(compute_delay(&config, 2), 1000);
        assert_eq!(compute_delay(&config, 3), 2000);
        assert_eq!(compute_delay(&config, 4), 4000);
    }

    #[test]
    fn compute_delay_capped() {
        let config = RetryConfig {
            max_retries: 10,
            base_delay_ms: 500,
            max_delay_ms: 5000,
            jitter: false,
        };
        assert_eq!(compute_delay(&config, 5), 5000);
    }
}
