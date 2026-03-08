use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use sqlx::PgPool;
use uuid::Uuid;

use super::VirtualKey;

// ---------------------------------------------------------------------------
// LRU Cache
// ---------------------------------------------------------------------------

struct CacheEntry {
    key: VirtualKey,
    inserted_at: Instant,
}

pub struct KeyCache {
    inner: tokio::sync::Mutex<LruInner>,
}

struct LruInner {
    map: HashMap<String, CacheEntry>,
    order: Vec<String>,
    capacity: usize,
    ttl: Duration,
}

impl KeyCache {
    pub fn new(capacity: usize, ttl_secs: u64) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(LruInner {
                map: HashMap::new(),
                order: Vec::new(),
                capacity,
                ttl: Duration::from_secs(ttl_secs),
            }),
        }
    }

    pub async fn get(&self, key_hash: &str) -> Option<VirtualKey> {
        let mut inner = self.inner.lock().await;
        let result = inner.map.get(key_hash).and_then(|entry| {
            if entry.inserted_at.elapsed() < inner.ttl {
                Some(entry.key.clone())
            } else {
                None
            }
        });

        if let Some(key) = result {
            // Move to back (most recently used)
            inner.order.retain(|k| k != key_hash);
            inner.order.push(key_hash.to_string());
            return Some(key);
        }

        // Remove expired entry if it existed
        if inner
            .map
            .get(key_hash)
            .is_some_and(|e| e.inserted_at.elapsed() >= inner.ttl)
        {
            inner.map.remove(key_hash);
            inner.order.retain(|k| k != key_hash);
        }

        None
    }

    pub async fn insert(&self, key_hash: String, key: VirtualKey) {
        let mut inner = self.inner.lock().await;
        // Evict if at capacity and key is not already present
        if !inner.map.contains_key(&key_hash)
            && inner.map.len() >= inner.capacity
            && let Some(oldest) = inner.order.first().cloned()
        {
            inner.map.remove(&oldest);
            inner.order.remove(0);
        }
        inner.order.retain(|k| k != &key_hash);
        inner.order.push(key_hash.clone());
        inner.map.insert(
            key_hash,
            CacheEntry {
                key,
                inserted_at: Instant::now(),
            },
        );
    }

    pub async fn invalidate(&self, key_hash: &str) {
        let mut inner = self.inner.lock().await;
        inner.map.remove(key_hash);
        inner.order.retain(|k| k != key_hash);
    }
}

// ---------------------------------------------------------------------------
// Postgres Repository
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
pub struct KeyRepository {
    pool: PgPool,
}

/// Parameters for creating a new virtual key.
#[cfg(feature = "postgres")]
#[derive(Debug)]
pub struct CreateKeyParams {
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub team_id: Option<String>,
    pub rpm_limit: Option<i32>,
    pub tpm_limit: Option<i32>,
    pub daily_budget_usd: Option<f64>,
    pub monthly_budget_usd: Option<f64>,
    pub budget_action: String,
    pub allowed_models: Vec<String>,
    pub metadata: serde_json::Value,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Parameters for updating an existing virtual key.
#[cfg(feature = "postgres")]
#[derive(Debug, Default)]
pub struct UpdateKeyParams {
    pub name: Option<String>,
    pub team_id: Option<Option<String>>,
    pub rpm_limit: Option<Option<i32>>,
    pub tpm_limit: Option<Option<i32>>,
    pub daily_budget_usd: Option<Option<f64>>,
    pub monthly_budget_usd: Option<Option<f64>>,
    pub budget_action: Option<String>,
    pub allowed_models: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

#[cfg(feature = "postgres")]
impl KeyRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn find_by_hash(&self, key_hash: &str) -> sqlx::Result<Option<VirtualKey>> {
        sqlx::query_as::<_, VirtualKey>(
            r#"SELECT id, name, key_hash, key_prefix, team_id, is_active,
                      rpm_limit, tpm_limit, daily_budget_usd, monthly_budget_usd,
                      budget_action, allowed_models, metadata, created_at, updated_at, expires_at
               FROM virtual_keys
               WHERE key_hash = $1 AND is_active = TRUE"#,
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn create(&self, params: CreateKeyParams) -> sqlx::Result<VirtualKey> {
        sqlx::query_as::<_, VirtualKey>(
            r#"INSERT INTO virtual_keys (name, key_hash, key_prefix, team_id, rpm_limit, tpm_limit,
                                         daily_budget_usd, monthly_budget_usd, budget_action,
                                         allowed_models, metadata, expires_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
               RETURNING id, name, key_hash, key_prefix, team_id, is_active,
                         rpm_limit, tpm_limit, daily_budget_usd, monthly_budget_usd,
                         budget_action, allowed_models, metadata, created_at, updated_at, expires_at"#,
        )
        .bind(&params.name)
        .bind(&params.key_hash)
        .bind(&params.key_prefix)
        .bind(&params.team_id)
        .bind(params.rpm_limit)
        .bind(params.tpm_limit)
        .bind(params.daily_budget_usd)
        .bind(params.monthly_budget_usd)
        .bind(&params.budget_action)
        .bind(&params.allowed_models)
        .bind(&params.metadata)
        .bind(params.expires_at)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn list(&self, limit: i64, offset: i64) -> sqlx::Result<Vec<VirtualKey>> {
        sqlx::query_as::<_, VirtualKey>(
            r#"SELECT id, name, key_hash, key_prefix, team_id, is_active,
                      rpm_limit, tpm_limit, daily_budget_usd, monthly_budget_usd,
                      budget_action, allowed_models, metadata, created_at, updated_at, expires_at
               FROM virtual_keys
               ORDER BY created_at DESC
               LIMIT $1 OFFSET $2"#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn revoke(&self, id: Uuid) -> sqlx::Result<bool> {
        let result = sqlx::query(
            "UPDATE virtual_keys SET is_active = FALSE, updated_at = NOW() WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update(
        &self,
        id: Uuid,
        params: UpdateKeyParams,
    ) -> sqlx::Result<Option<VirtualKey>> {
        // Build dynamic update — we use COALESCE-style approach with individual SET clauses
        // For simplicity we update all nullable fields using the provided values
        let row = sqlx::query_as::<_, VirtualKey>(
            r#"UPDATE virtual_keys SET
                name = COALESCE($2, name),
                team_id = CASE WHEN $3::boolean THEN $4 ELSE team_id END,
                rpm_limit = CASE WHEN $5::boolean THEN $6 ELSE rpm_limit END,
                tpm_limit = CASE WHEN $7::boolean THEN $8 ELSE tpm_limit END,
                daily_budget_usd = CASE WHEN $9::boolean THEN $10 ELSE daily_budget_usd END,
                monthly_budget_usd = CASE WHEN $11::boolean THEN $12 ELSE monthly_budget_usd END,
                budget_action = COALESCE($13, budget_action),
                allowed_models = COALESCE($14, allowed_models),
                metadata = COALESCE($15, metadata),
                expires_at = CASE WHEN $16::boolean THEN $17 ELSE expires_at END,
                updated_at = NOW()
               WHERE id = $1
               RETURNING id, name, key_hash, key_prefix, team_id, is_active,
                         rpm_limit, tpm_limit, daily_budget_usd, monthly_budget_usd,
                         budget_action, allowed_models, metadata, created_at, updated_at, expires_at"#,
        )
        .bind(id)
        .bind(params.name)
        .bind(params.team_id.is_some())
        .bind(params.team_id.flatten())
        .bind(params.rpm_limit.is_some())
        .bind(params.rpm_limit.flatten())
        .bind(params.tpm_limit.is_some())
        .bind(params.tpm_limit.flatten())
        .bind(params.daily_budget_usd.is_some())
        .bind(params.daily_budget_usd.flatten())
        .bind(params.monthly_budget_usd.is_some())
        .bind(params.monthly_budget_usd.flatten())
        .bind(params.budget_action)
        .bind(params.allowed_models)
        .bind(params.metadata)
        .bind(params.expires_at.is_some())
        .bind(params.expires_at.flatten())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Rotate a key: create new key with same config, expire old key after grace period.
    pub async fn rotate_key(
        &self,
        id: Uuid,
        new_key_hash: &str,
        new_key_prefix: &str,
        grace_period_hours: i64,
    ) -> sqlx::Result<Option<VirtualKey>> {
        let old_key = self.find_by_id(id).await?;
        let old_key = match old_key {
            Some(k) => k,
            None => return Ok(None),
        };

        // Set old key to expire after grace period
        let grace_expiry = chrono::Utc::now() + chrono::Duration::hours(grace_period_hours);
        sqlx::query("UPDATE virtual_keys SET expires_at = $2, updated_at = NOW() WHERE id = $1")
            .bind(id)
            .bind(grace_expiry)
            .execute(&self.pool)
            .await?;

        // Create new key with same configuration
        let new_key = sqlx::query_as::<_, VirtualKey>(
            r#"INSERT INTO virtual_keys (name, key_hash, key_prefix, team_id, rpm_limit, tpm_limit,
                                         daily_budget_usd, monthly_budget_usd, budget_action,
                                         allowed_models, metadata, rotation_interval_days, rotated_from, grace_period_hours)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
               RETURNING id, name, key_hash, key_prefix, team_id, is_active,
                         rpm_limit, tpm_limit, daily_budget_usd, monthly_budget_usd,
                         budget_action, allowed_models, metadata, created_at, updated_at, expires_at"#,
        )
        .bind(&old_key.name)
        .bind(new_key_hash)
        .bind(new_key_prefix)
        .bind(&old_key.team_id)
        .bind(old_key.rpm_limit)
        .bind(old_key.tpm_limit)
        .bind(old_key.daily_budget_usd)
        .bind(old_key.monthly_budget_usd)
        .bind(&old_key.budget_action)
        .bind(&old_key.allowed_models)
        .bind(&old_key.metadata)
        .bind::<Option<i32>>(None) // rotation_interval_days from old key (not in select)
        .bind(id) // rotated_from
        .bind(grace_period_hours as i32)
        .fetch_one(&self.pool)
        .await?;

        Ok(Some(new_key))
    }

    /// Find a key by its ID (regardless of active status).
    pub async fn find_by_id(&self, id: Uuid) -> sqlx::Result<Option<VirtualKey>> {
        sqlx::query_as::<_, VirtualKey>(
            r#"SELECT id, name, key_hash, key_prefix, team_id, is_active,
                      rpm_limit, tpm_limit, daily_budget_usd, monthly_budget_usd,
                      budget_action, allowed_models, metadata, created_at, updated_at, expires_at
               FROM virtual_keys
               WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cache_insert_and_get() {
        let cache = KeyCache::new(10, 300);
        let key = VirtualKey {
            id: Uuid::new_v4(),
            name: "test".into(),
            key_hash: "hash1".into(),
            key_prefix: "prism_ab".into(),
            team_id: None,
            is_active: true,
            rpm_limit: None,
            tpm_limit: None,
            daily_budget_usd: None,
            monthly_budget_usd: None,
            budget_action: "reject".into(),
            allowed_models: vec![],
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            expires_at: None,
            session_budget_usd: None,
        };
        cache.insert("hash1".into(), key.clone()).await;
        let got = cache.get("hash1").await;
        assert!(got.is_some());
        assert_eq!(got.unwrap().name, "test");
    }

    #[tokio::test]
    async fn cache_miss() {
        let cache = KeyCache::new(10, 300);
        assert!(cache.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn cache_invalidate() {
        let cache = KeyCache::new(10, 300);
        let key = VirtualKey {
            id: Uuid::new_v4(),
            name: "test".into(),
            key_hash: "hash1".into(),
            key_prefix: "prism_ab".into(),
            team_id: None,
            is_active: true,
            rpm_limit: None,
            tpm_limit: None,
            daily_budget_usd: None,
            monthly_budget_usd: None,
            budget_action: "reject".into(),
            allowed_models: vec![],
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            expires_at: None,
            session_budget_usd: None,
        };
        cache.insert("hash1".into(), key).await;
        cache.invalidate("hash1").await;
        assert!(cache.get("hash1").await.is_none());
    }

    #[tokio::test]
    async fn cache_eviction() {
        let cache = KeyCache::new(2, 300);
        for i in 0..3 {
            let key = VirtualKey {
                id: Uuid::new_v4(),
                name: format!("key{i}"),
                key_hash: format!("hash{i}"),
                key_prefix: "prism_ab".into(),
                team_id: None,
                is_active: true,
                rpm_limit: None,
                tpm_limit: None,
                daily_budget_usd: None,
                monthly_budget_usd: None,
                budget_action: "reject".into(),
                allowed_models: vec![],
                metadata: serde_json::json!({}),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                expires_at: None,
                session_budget_usd: None,
            };
            cache.insert(format!("hash{i}"), key).await;
        }
        // First key should have been evicted
        assert!(cache.get("hash0").await.is_none());
        assert!(cache.get("hash1").await.is_some());
        assert!(cache.get("hash2").await.is_some());
    }

    #[tokio::test]
    async fn cache_ttl_expiry() {
        let cache = KeyCache::new(10, 0); // 0-second TTL
        let key = VirtualKey {
            id: Uuid::new_v4(),
            name: "test".into(),
            key_hash: "hash1".into(),
            key_prefix: "prism_ab".into(),
            team_id: None,
            is_active: true,
            rpm_limit: None,
            tpm_limit: None,
            daily_budget_usd: None,
            monthly_budget_usd: None,
            budget_action: "reject".into(),
            allowed_models: vec![],
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            expires_at: None,
            session_budget_usd: None,
        };
        cache.insert("hash1".into(), key).await;
        // With 0s TTL, the entry should be expired immediately
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(cache.get("hash1").await.is_none());
    }
}
