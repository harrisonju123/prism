pub mod budget;
pub mod jwt;
pub mod rate_limit;
pub mod virtual_key;

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::PrismError;

use self::virtual_key::{KeyCache, KeyRepository};

// ---------------------------------------------------------------------------
// Domain model
// ---------------------------------------------------------------------------

/// A virtual API key (domain model, mapped from Postgres rows).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct VirtualKey {
    pub id: Uuid,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub team_id: Option<String>,
    pub is_active: bool,
    pub rpm_limit: Option<i32>,
    pub tpm_limit: Option<i32>,
    pub daily_budget_usd: Option<f64>,
    pub monthly_budget_usd: Option<f64>,
    pub budget_action: String,
    pub allowed_models: Vec<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// AuthContext — extracted identity for an authenticated request
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub key_id: Uuid,
    pub key_hash: String,
    pub key_prefix: String,
    pub team_id: Option<String>,
    pub rpm_limit: Option<u32>,
    pub tpm_limit: Option<u32>,
    pub daily_budget_usd: Option<f64>,
    pub monthly_budget_usd: Option<f64>,
    pub budget_action: budget::BudgetAction,
    pub allowed_models: Vec<String>,
}

// ---------------------------------------------------------------------------
// MaybeAuth — axum extractor
// ---------------------------------------------------------------------------

/// Axum extractor that optionally authenticates a request via virtual key.
///
/// - If `KeyService` is not in extensions (keys disabled) → `MaybeAuth(None)`.
/// - If keys enabled → extracts `Authorization: Bearer prism_<32hex>`, validates it.
/// - Returns 401 on missing, invalid, expired, or revoked key.
#[derive(Debug, Clone)]
pub struct MaybeAuth(pub Option<AuthContext>);

impl<S: Send + Sync> FromRequestParts<S> for MaybeAuth {
    type Rejection = PrismError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Check if KeyService is available in extensions
        let key_service = match parts.extensions.get::<Arc<KeyService>>() {
            Some(ks) => ks.clone(),
            None => return Ok(MaybeAuth(None)), // Keys not enabled — passthrough
        };

        // Extract the Authorization header
        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(PrismError::Unauthorized)?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(PrismError::Unauthorized)?;

        // Validate key format: prism_<32 hex chars>
        if !is_valid_key_format(token) {
            return Err(PrismError::Unauthorized);
        }

        // Hash the plaintext key
        let key_hash = hash_key(token);

        // Look up in cache / DB
        let vk = key_service
            .validate_key(&key_hash)
            .await
            .map_err(|_| PrismError::Internal("key lookup failed".into()))?
            .ok_or(PrismError::Unauthorized)?;

        // Check expiration
        if let Some(expires) = vk.expires_at
            && expires < Utc::now()
        {
            return Err(PrismError::Unauthorized);
        }

        Ok(MaybeAuth(Some(AuthContext {
            key_id: vk.id,
            key_hash: vk.key_hash,
            key_prefix: vk.key_prefix,
            team_id: vk.team_id,
            rpm_limit: vk.rpm_limit.map(|v| v as u32),
            tpm_limit: vk.tpm_limit.map(|v| v as u32),
            daily_budget_usd: vk.daily_budget_usd,
            monthly_budget_usd: vk.monthly_budget_usd,
            budget_action: budget::BudgetAction::from_str_lossy(&vk.budget_action),
            allowed_models: vk.allowed_models,
        })))
    }
}

// ---------------------------------------------------------------------------
// MasterAuth — axum extractor for management API
// ---------------------------------------------------------------------------

/// Axum extractor that validates the master key for management endpoints.
pub struct MasterAuth;

impl<S: Send + Sync> FromRequestParts<S> for MasterAuth {
    type Rejection = PrismError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let master_key = parts
            .extensions
            .get::<MasterKey>()
            .ok_or(PrismError::Unauthorized)?;

        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(PrismError::Unauthorized)?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(PrismError::Unauthorized)?;

        if token != master_key.0 {
            return Err(PrismError::Unauthorized);
        }

        Ok(MasterAuth)
    }
}

/// Wrapper for the master key, stored in request extensions.
#[derive(Debug, Clone)]
pub struct MasterKey(pub String);

// ---------------------------------------------------------------------------
// KeyService — coordinates cache + repository
// ---------------------------------------------------------------------------

pub struct KeyService {
    cache: KeyCache,
    repo: KeyRepository,
}

impl KeyService {
    pub fn new(repo: KeyRepository, cache_capacity: usize) -> Self {
        Self {
            cache: KeyCache::new(cache_capacity, 300), // 5-minute TTL
            repo,
        }
    }

    /// Validate a key hash: check cache first, then DB.
    pub async fn validate_key(&self, key_hash: &str) -> anyhow::Result<Option<VirtualKey>> {
        // Check cache
        if let Some(vk) = self.cache.get(key_hash).await {
            return Ok(Some(vk));
        }

        // Fall back to DB
        let vk = self
            .repo
            .find_by_hash(key_hash)
            .await
            .map_err(|e| anyhow::anyhow!("db error: {e}"))?;

        if let Some(ref vk) = vk {
            self.cache.insert(key_hash.to_string(), vk.clone()).await;
        }

        Ok(vk)
    }

    /// Invalidate a key from the cache (called on revoke/update).
    pub async fn invalidate_cache(&self, key_hash: &str) {
        self.cache.invalidate(key_hash).await;
    }

    pub fn repo(&self) -> &KeyRepository {
        &self.repo
    }
}

// ---------------------------------------------------------------------------
// Key generation + hashing helpers
// ---------------------------------------------------------------------------

/// Generate a new virtual key: `prism_<32 random hex>` (16 random bytes).
pub fn generate_key() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 16] = rng.random();
    format!("prism_{}", hex::encode(bytes))
}

/// SHA-256 hash of a plaintext key.
pub fn hash_key(plaintext: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(plaintext.as_bytes());
    hex::encode(hasher.finalize())
}

/// Check if a key has the expected format: `prism_` followed by exactly 32 hex chars.
pub fn is_valid_key_format(key: &str) -> bool {
    if let Some(hex_part) = key.strip_prefix("prism_") {
        hex_part.len() == 32 && hex_part.chars().all(|c| c.is_ascii_hexdigit())
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_format_validation() {
        let key = generate_key();
        assert!(is_valid_key_format(&key));
        assert!(!is_valid_key_format("sk-1234"));
        assert!(!is_valid_key_format("prism_short"));
        assert!(!is_valid_key_format(
            "prism_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
        )); // non-hex
    }

    #[test]
    fn hash_determinism() {
        let key = "prism_aabbccdd11223344aabbccdd11223344";
        let h1 = hash_key(key);
        let h2 = hash_key(key);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }
}
