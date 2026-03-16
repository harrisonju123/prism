use std::collections::VecDeque;
use std::time::{Duration, Instant};

use dashmap::DashMap;

#[derive(Debug, Clone)]
pub struct RateLimitResult {
    pub allowed: bool,
    pub retry_after_secs: Option<u64>,
}

const WINDOW_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// RateLimiter — dispatches to InMemory or Redis backend
// ---------------------------------------------------------------------------

pub enum RateLimiter {
    InMemory(InMemoryRateLimiter),
    #[cfg(feature = "redis-backend")]
    Redis(RedisRateLimiter),
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::InMemory(InMemoryRateLimiter::new())
    }

    #[cfg(feature = "redis-backend")]
    pub async fn new_redis(redis_url: &str) -> Self {
        Self::Redis(RedisRateLimiter::new(redis_url).await)
    }

    pub async fn check_rpm(&self, key_hash: &str, limit: u32) -> RateLimitResult {
        match self {
            Self::InMemory(inner) => inner.check_rpm(key_hash, limit),
            #[cfg(feature = "redis-backend")]
            Self::Redis(inner) => inner.check_rpm(key_hash, limit).await,
        }
    }

    pub async fn check_tpm(&self, key_hash: &str, limit: u32) -> RateLimitResult {
        match self {
            Self::InMemory(inner) => inner.check_tpm(key_hash, limit),
            #[cfg(feature = "redis-backend")]
            Self::Redis(inner) => inner.check_tpm(key_hash, limit).await,
        }
    }

    pub async fn record_request(&self, key_hash: &str) {
        match self {
            Self::InMemory(inner) => inner.record_request(key_hash),
            #[cfg(feature = "redis-backend")]
            Self::Redis(inner) => inner.record_request(key_hash).await,
        }
    }

    pub async fn record_tokens(&self, key_hash: &str, count: u32) {
        match self {
            Self::InMemory(inner) => inner.record_tokens(key_hash, count),
            #[cfg(feature = "redis-backend")]
            Self::Redis(inner) => inner.record_tokens(key_hash, count).await,
        }
    }

    pub fn prune_expired(&self) {
        match self {
            Self::InMemory(inner) => inner.prune_expired(),
            #[cfg(feature = "redis-backend")]
            Self::Redis(_) => {} // Redis handles TTL automatically
        }
    }

    pub fn current_rpm(&self, key_hash: &str) -> usize {
        match self {
            Self::InMemory(inner) => inner.current_rpm(key_hash),
            #[cfg(feature = "redis-backend")]
            Self::Redis(inner) => inner.fallback.current_rpm(key_hash),
        }
    }

    pub fn current_tpm(&self, key_hash: &str) -> u32 {
        match self {
            Self::InMemory(inner) => inner.current_tpm(key_hash),
            #[cfg(feature = "redis-backend")]
            Self::Redis(inner) => inner.fallback.current_tpm(key_hash),
        }
    }
}

// ---------------------------------------------------------------------------
// In-memory backend (existing implementation)
// ---------------------------------------------------------------------------

pub struct InMemoryRateLimiter {
    rpm: DashMap<String, VecDeque<Instant>>,
    tpm: DashMap<String, VecDeque<(Instant, u32)>>,
}

impl InMemoryRateLimiter {
    pub fn new() -> Self {
        Self {
            rpm: DashMap::new(),
            tpm: DashMap::new(),
        }
    }

    pub fn check_rpm(&self, key_hash: &str, limit: u32) -> RateLimitResult {
        let now = Instant::now();
        let cutoff = now - std::time::Duration::from_secs(WINDOW_SECS);

        let mut entry = self.rpm.entry(key_hash.to_string()).or_default();
        while entry.front().is_some_and(|&t| t < cutoff) {
            entry.pop_front();
        }

        if entry.len() as u32 >= limit {
            let retry_after = entry.front().map(|oldest| {
                WINDOW_SECS.saturating_sub(now.duration_since(*oldest).as_secs())
            }).unwrap_or(1);
            RateLimitResult {
                allowed: false,
                retry_after_secs: Some(retry_after.max(1)),
            }
        } else {
            RateLimitResult {
                allowed: true,
                retry_after_secs: None,
            }
        }
    }

    pub fn check_tpm(&self, key_hash: &str, limit: u32) -> RateLimitResult {
        let now = Instant::now();
        let cutoff = now - std::time::Duration::from_secs(WINDOW_SECS);

        let mut entry = self.tpm.entry(key_hash.to_string()).or_default();
        while entry.front().is_some_and(|(t, _)| *t < cutoff) {
            entry.pop_front();
        }

        let total_tokens: u32 = entry.iter().map(|(_, count)| count).sum();
        if total_tokens >= limit {
            let retry_after = entry.front().map(|oldest| {
                WINDOW_SECS.saturating_sub(now.duration_since(oldest.0).as_secs())
            }).unwrap_or(1);
            RateLimitResult {
                allowed: false,
                retry_after_secs: Some(retry_after.max(1)),
            }
        } else {
            RateLimitResult {
                allowed: true,
                retry_after_secs: None,
            }
        }
    }

    pub fn record_request(&self, key_hash: &str) {
        self.rpm
            .entry(key_hash.to_string())
            .or_default()
            .push_back(Instant::now());
    }

    pub fn record_tokens(&self, key_hash: &str, count: u32) {
        if count > 0 {
            self.tpm
                .entry(key_hash.to_string())
                .or_default()
                .push_back((Instant::now(), count));
        }
    }

    pub fn current_rpm(&self, key_hash: &str) -> usize {
        let cutoff = Instant::now() - Duration::from_secs(WINDOW_SECS);
        self.rpm
            .get(key_hash)
            .map(|e| e.iter().filter(|&&t| t >= cutoff).count())
            .unwrap_or(0)
    }

    pub fn current_tpm(&self, key_hash: &str) -> u32 {
        let cutoff = Instant::now() - Duration::from_secs(WINDOW_SECS);
        self.tpm
            .get(key_hash)
            .map(|e| {
                e.iter()
                    .filter(|(t, _)| *t >= cutoff)
                    .map(|(_, c)| *c)
                    .sum()
            })
            .unwrap_or(0)
    }

    pub fn prune_expired(&self) {
        let now = Instant::now();
        let cutoff = now - std::time::Duration::from_secs(WINDOW_SECS);

        self.rpm.retain(|_, deque| {
            while deque.front().is_some_and(|&t| t < cutoff) {
                deque.pop_front();
            }
            !deque.is_empty()
        });

        self.tpm.retain(|_, deque| {
            while deque.front().is_some_and(|(t, _)| *t < cutoff) {
                deque.pop_front();
            }
            !deque.is_empty()
        });
    }
}

// ---------------------------------------------------------------------------
// Redis backend — uses sorted sets for sliding window per key hash
// ---------------------------------------------------------------------------

#[cfg(feature = "redis-backend")]
pub struct RedisRateLimiter {
    conn: Option<redis::aio::MultiplexedConnection>,
    fallback: InMemoryRateLimiter,
}

#[cfg(feature = "redis-backend")]
impl RedisRateLimiter {
    pub async fn new(redis_url: &str) -> Self {
        let conn = match redis::Client::open(redis_url) {
            Ok(client) => match client.get_multiplexed_async_connection().await {
                Ok(conn) => {
                    tracing::info!(%redis_url, "redis rate limiter connected");
                    Some(conn)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "redis rate limiter connection failed, using fallback");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "redis rate limiter client error, using fallback");
                None
            }
        };
        Self {
            conn,
            fallback: InMemoryRateLimiter::new(),
        }
    }

    pub async fn check_rpm(&self, key_hash: &str, limit: u32) -> RateLimitResult {
        if let Some(ref conn) = self.conn {
            match self
                .check_sorted_set(conn, &format!("ratelimit:rpm:{key_hash}"), limit)
                .await
            {
                Ok(result) => return result,
                Err(e) => {
                    tracing::warn!(error = %e, "redis check_rpm failed, using fallback");
                }
            }
        }
        self.fallback.check_rpm(key_hash, limit)
    }

    pub async fn check_tpm(&self, key_hash: &str, limit: u32) -> RateLimitResult {
        if let Some(ref conn) = self.conn {
            match self
                .check_tpm_sorted_set(conn, &format!("ratelimit:tpm:{key_hash}"), limit)
                .await
            {
                Ok(result) => return result,
                Err(e) => {
                    tracing::warn!(error = %e, "redis check_tpm failed, using fallback");
                }
            }
        }
        self.fallback.check_tpm(key_hash, limit)
    }

    async fn check_tpm_sorted_set(
        &self,
        conn: &redis::aio::MultiplexedConnection,
        key: &str,
        limit: u32,
    ) -> Result<RateLimitResult, redis::RedisError> {
        let mut conn = conn.clone();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let cutoff_ms = now_ms - (WINDOW_SECS as i64 * 1000);

        // Atomically prune expired entries and fetch all remaining members
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("ZREMRANGEBYSCORE")
            .arg(key)
            .arg("-inf")
            .arg(cutoff_ms)
            .ignore()
            .cmd("ZRANGE")
            .arg(key)
            .arg(0isize)
            .arg(-1isize);

        let (members,): (Vec<String>,) = pipe.query_async(&mut conn).await?;

        // Member format: "{timestamp_ms}:{token_count}:{random_u32}"
        // Sum the middle field across all entries in the current window
        let total_tokens: u32 = members
            .iter()
            .filter_map(|m| m.split(':').nth(1)?.parse::<u32>().ok())
            .sum();

        if total_tokens >= limit {
            let oldest: Vec<(String, f64)> = redis::cmd("ZRANGE")
                .arg(key)
                .arg(0isize)
                .arg(0isize)
                .arg("WITHSCORES")
                .query_async(&mut conn)
                .await?;

            let retry_after = oldest
                .first()
                .map(|(_, score)| {
                    let expires_at_ms = *score as i64 + (WINDOW_SECS as i64 * 1000);
                    (((expires_at_ms - now_ms).max(0) / 1000) as u64).saturating_add(1)
                })
                .unwrap_or(1);

            return Ok(RateLimitResult {
                allowed: false,
                retry_after_secs: Some(retry_after),
            });
        }

        Ok(RateLimitResult {
            allowed: true,
            retry_after_secs: None,
        })
    }

    async fn check_sorted_set(
        &self,
        conn: &redis::aio::MultiplexedConnection,
        key: &str,
        limit: u32,
    ) -> Result<RateLimitResult, redis::RedisError> {
        let mut conn = conn.clone();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let cutoff_ms = now_ms - (WINDOW_SECS as i64 * 1000);

        // Remove expired entries and count remaining in one pipeline
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("ZREMRANGEBYSCORE")
            .arg(key)
            .arg("-inf")
            .arg(cutoff_ms)
            .ignore()
            .cmd("ZCARD")
            .arg(key);

        let (count,): (u32,) = pipe.query_async(&mut conn).await?;

        if count >= limit {
            // Get the oldest entry to calculate retry_after
            let oldest: Vec<(String, f64)> = redis::cmd("ZRANGE")
                .arg(key)
                .arg(0i64)
                .arg(0i64)
                .arg("WITHSCORES")
                .query_async(&mut conn)
                .await?;

            let retry_after = if let Some((_, score)) = oldest.first() {
                let oldest_ms = *score as i64;
                let expire_at = oldest_ms + (WINDOW_SECS as i64 * 1000);
                ((expire_at - now_ms).max(1000) / 1000) as u64
            } else {
                1
            };

            Ok(RateLimitResult {
                allowed: false,
                retry_after_secs: Some(retry_after),
            })
        } else {
            Ok(RateLimitResult {
                allowed: true,
                retry_after_secs: None,
            })
        }
    }

    pub async fn record_request(&self, key_hash: &str) {
        if let Some(ref conn) = self.conn {
            let key = format!("ratelimit:rpm:{key_hash}");
            if let Err(e) = self.add_to_sorted_set(conn, &key, 1).await {
                tracing::warn!(error = %e, "redis record_request failed, using fallback");
                self.fallback.record_request(key_hash);
                return;
            }
        } else {
            self.fallback.record_request(key_hash);
        }
    }

    pub async fn record_tokens(&self, key_hash: &str, count: u32) {
        if count == 0 {
            return;
        }
        if let Some(ref conn) = self.conn {
            let key = format!("ratelimit:tpm:{key_hash}");
            if let Err(e) = self.add_to_sorted_set(conn, &key, count).await {
                tracing::warn!(error = %e, "redis record_tokens failed, using fallback");
                self.fallback.record_tokens(key_hash, count);
                return;
            }
        } else {
            self.fallback.record_tokens(key_hash, count);
        }
    }

    async fn add_to_sorted_set(
        &self,
        conn: &redis::aio::MultiplexedConnection,
        key: &str,
        value: u32,
    ) -> Result<(), redis::RedisError> {
        let mut conn = conn.clone();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let cutoff_ms = now_ms - (WINDOW_SECS as i64 * 1000);
        // Use timestamp + random suffix as member to avoid dedup, encode value in member
        let member = format!("{now_ms}:{value}:{}", rand::random::<u32>());

        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("ZADD")
            .arg(key)
            .arg(now_ms as f64)
            .arg(&member)
            .ignore()
            .cmd("ZREMRANGEBYSCORE")
            .arg(key)
            .arg("-inf")
            .arg(cutoff_ms)
            .ignore()
            .cmd("EXPIRE")
            .arg(key)
            .arg(WINDOW_SECS * 2)
            .ignore();

        pipe.query_async::<()>(&mut conn).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn rpm_allows_under_limit() {
        let rl = RateLimiter::new();
        let res = rl.check_rpm("k1", 5).await;
        assert!(res.allowed);
    }

    #[tokio::test]
    async fn rpm_blocks_at_limit() {
        let rl = RateLimiter::new();
        for _ in 0..5 {
            rl.record_request("k1").await;
        }
        let res = rl.check_rpm("k1", 5).await;
        assert!(!res.allowed);
        assert!(res.retry_after_secs.is_some());
    }

    #[tokio::test]
    async fn tpm_allows_under_limit() {
        let rl = RateLimiter::new();
        rl.record_tokens("k1", 100).await;
        let res = rl.check_tpm("k1", 500).await;
        assert!(res.allowed);
    }

    #[tokio::test]
    async fn tpm_blocks_at_limit() {
        let rl = RateLimiter::new();
        rl.record_tokens("k1", 500).await;
        let res = rl.check_tpm("k1", 500).await;
        assert!(!res.allowed);
    }

    #[test]
    fn rpm_window_slides() {
        let rl = InMemoryRateLimiter::new();
        {
            let mut entry = rl.rpm.entry("k1".to_string()).or_default();
            entry.push_back(Instant::now() - Duration::from_secs(61));
        }
        let res = rl.check_rpm("k1", 1);
        assert!(res.allowed);
    }

    #[test]
    fn tpm_window_slides() {
        let rl = InMemoryRateLimiter::new();
        {
            let mut entry = rl.tpm.entry("k1".to_string()).or_default();
            entry.push_back((Instant::now() - Duration::from_secs(61), 1000));
        }
        let res = rl.check_tpm("k1", 500);
        assert!(res.allowed);
    }

    #[tokio::test]
    async fn concurrent_access() {
        use std::sync::Arc;
        let rl = Arc::new(RateLimiter::new());
        let mut handles = Vec::new();
        for _ in 0..10 {
            let rl = rl.clone();
            handles.push(tokio::spawn(async move {
                rl.record_request("k1").await;
                rl.check_rpm("k1", 100).await;
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    }

    #[test]
    fn prune_removes_stale() {
        let rl = RateLimiter::new();
        if let RateLimiter::InMemory(ref inner) = rl {
            {
                let mut entry = inner.rpm.entry("k1".to_string()).or_default();
                entry.push_back(Instant::now() - Duration::from_secs(120));
            }
            {
                let mut entry = inner.tpm.entry("k1".to_string()).or_default();
                entry.push_back((Instant::now() - Duration::from_secs(120), 100));
            }
            rl.prune_expired();
            assert!(!inner.rpm.contains_key("k1"));
            assert!(!inner.tpm.contains_key("k1"));
        }
    }

    #[cfg(feature = "redis-backend")]
    #[tokio::test]
    async fn redis_fallback_works() {
        // With no Redis available, should fall back gracefully
        let rl = RateLimiter::new_redis("redis://localhost:1").await;
        rl.record_request("k1").await;
        let res = rl.check_rpm("k1", 5).await;
        assert!(res.allowed);
    }

    #[test]
    fn tpm_member_parsing_sums_token_counts() {
        let members = vec![
            "1700000000000:400:11111111".to_string(),
            "1700000001000:300:22222222".to_string(),
            "1700000002000:200:33333333".to_string(),
        ];
        let total: u32 = members
            .iter()
            .filter_map(|m| m.split(':').nth(1)?.parse::<u32>().ok())
            .sum();
        assert_eq!(total, 900);
    }

    #[test]
    fn tpm_member_parsing_ignores_malformed() {
        let members = vec![
            "1700000000000:512:99999999".to_string(),
            "bad_entry".to_string(),
            "1700000001000:notanumber:1".to_string(),
        ];
        let total: u32 = members
            .iter()
            .filter_map(|m| m.split(':').nth(1)?.parse::<u32>().ok())
            .sum();
        assert_eq!(total, 512);
    }

    #[tokio::test]
    async fn record_tokens_zero_is_noop() {
        let rl = RateLimiter::new();
        rl.record_tokens("k1", 0).await;
        let res = rl.check_tpm("k1", 1).await;
        assert!(res.allowed);
    }

    #[tokio::test]
    async fn rpm_multiple_keys_independent() {
        let rl = RateLimiter::new();
        for _ in 0..5 {
            rl.record_request("k1").await;
        }
        let res_k1 = rl.check_rpm("k1", 5).await;
        let res_k2 = rl.check_rpm("k2", 5).await;
        assert!(!res_k1.allowed);
        assert!(res_k2.allowed);
    }
}
