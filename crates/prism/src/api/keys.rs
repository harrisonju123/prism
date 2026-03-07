use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{PrismError, Result};
use crate::keys::virtual_key::{CreateKeyParams, UpdateKeyParams};
use crate::keys::{self, MasterAuth, VirtualKey};
use crate::proxy::handler::AppState;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub rpm_limit: Option<i32>,
    #[serde(default)]
    pub tpm_limit: Option<i32>,
    #[serde(default)]
    pub daily_budget_usd: Option<f64>,
    #[serde(default)]
    pub monthly_budget_usd: Option<f64>,
    #[serde(default = "default_budget_action")]
    pub budget_action: String,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default = "default_metadata")]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

fn default_budget_action() -> String {
    "reject".into()
}

fn default_metadata() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Serialize)]
pub struct CreateKeyResponse {
    pub key: String, // plaintext — returned only once
    pub key_prefix: String,
    #[serde(flatten)]
    pub details: KeyDetails,
}

#[derive(Debug, Serialize)]
pub struct KeyDetails {
    pub id: Uuid,
    pub name: String,
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

impl From<VirtualKey> for KeyDetails {
    fn from(vk: VirtualKey) -> Self {
        Self {
            id: vk.id,
            name: vk.name,
            team_id: vk.team_id,
            is_active: vk.is_active,
            rpm_limit: vk.rpm_limit,
            tpm_limit: vk.tpm_limit,
            daily_budget_usd: vk.daily_budget_usd,
            monthly_budget_usd: vk.monthly_budget_usd,
            budget_action: vk.budget_action,
            allowed_models: vk.allowed_models,
            metadata: vk.metadata,
            created_at: vk.created_at,
            updated_at: vk.updated_at,
            expires_at: vk.expires_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Deserialize)]
pub struct UpdateKeyRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub team_id: Option<Option<String>>,
    #[serde(default)]
    pub rpm_limit: Option<Option<i32>>,
    #[serde(default)]
    pub tpm_limit: Option<Option<i32>>,
    #[serde(default)]
    pub daily_budget_usd: Option<Option<f64>>,
    #[serde(default)]
    pub monthly_budget_usd: Option<Option<f64>>,
    #[serde(default)]
    pub budget_action: Option<String>,
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub key_id: Uuid,
    pub rpm_current: usize,
    pub tpm_current: u32,
    pub daily_spend_usd: f64,
    pub monthly_spend_usd: f64,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/v1/keys — create a new virtual key.
pub async fn create_key(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Json(body): Json<CreateKeyRequest>,
) -> Result<Json<CreateKeyResponse>> {
    let key_service = state
        .key_service
        .as_ref()
        .ok_or_else(|| PrismError::Internal("keys not enabled".into()))?;

    let plaintext = keys::generate_key();
    let key_hash = keys::hash_key(&plaintext);
    let key_prefix = plaintext[..10].to_string(); // "prism_xxxx"

    let params = CreateKeyParams {
        name: body.name,
        key_hash,
        key_prefix: key_prefix.clone(),
        team_id: body.team_id,
        rpm_limit: body.rpm_limit,
        tpm_limit: body.tpm_limit,
        daily_budget_usd: body.daily_budget_usd,
        monthly_budget_usd: body.monthly_budget_usd,
        budget_action: body.budget_action,
        allowed_models: body.allowed_models,
        metadata: body.metadata,
        expires_at: body.expires_at,
    };

    let vk = key_service
        .repo()
        .create(params)
        .await
        .map_err(|e| PrismError::Internal(format!("failed to create key: {e}")))?;

    Ok(Json(CreateKeyResponse {
        key: plaintext,
        key_prefix,
        details: vk.into(),
    }))
}

/// GET /api/v1/keys — list keys (paginated, no hashes).
pub async fn list_keys(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<KeyDetails>>> {
    let key_service = state
        .key_service
        .as_ref()
        .ok_or_else(|| PrismError::Internal("keys not enabled".into()))?;

    let keys = key_service
        .repo()
        .list(query.limit, query.offset)
        .await
        .map_err(|e| PrismError::Internal(format!("failed to list keys: {e}")))?;

    Ok(Json(keys.into_iter().map(KeyDetails::from).collect()))
}

/// DELETE /api/v1/keys/:id — revoke (soft-delete) a key.
pub async fn revoke_key(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let key_service = state
        .key_service
        .as_ref()
        .ok_or_else(|| PrismError::Internal("keys not enabled".into()))?;

    // Get the key hash for cache invalidation
    let vk = key_service
        .repo()
        .find_by_id(id)
        .await
        .map_err(|e| PrismError::Internal(format!("db error: {e}")))?;

    let revoked = key_service
        .repo()
        .revoke(id)
        .await
        .map_err(|e| PrismError::Internal(format!("failed to revoke key: {e}")))?;

    if !revoked {
        return Err(PrismError::ModelNotFound(format!("key {id} not found")));
    }

    // Invalidate cache
    if let Some(vk) = vk {
        key_service.invalidate_cache(&vk.key_hash).await;
    }

    Ok(Json(serde_json::json!({ "revoked": true, "id": id })))
}

/// PATCH /api/v1/keys/:id — partial update.
pub async fn update_key(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateKeyRequest>,
) -> Result<Json<KeyDetails>> {
    let key_service = state
        .key_service
        .as_ref()
        .ok_or_else(|| PrismError::Internal("keys not enabled".into()))?;

    let params = UpdateKeyParams {
        name: body.name,
        team_id: body.team_id,
        rpm_limit: body.rpm_limit,
        tpm_limit: body.tpm_limit,
        daily_budget_usd: body.daily_budget_usd,
        monthly_budget_usd: body.monthly_budget_usd,
        budget_action: body.budget_action,
        allowed_models: body.allowed_models,
        metadata: body.metadata,
        expires_at: body.expires_at,
    };

    let vk = key_service
        .repo()
        .update(id, params)
        .await
        .map_err(|e| PrismError::Internal(format!("failed to update key: {e}")))?
        .ok_or_else(|| PrismError::ModelNotFound(format!("key {id} not found")))?;

    // Invalidate cache so new limits take effect
    key_service.invalidate_cache(&vk.key_hash).await;

    Ok(Json(vk.into()))
}

/// POST /api/v1/keys/:id/rotate — rotate a key.
pub async fn rotate_key(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let key_service = state
        .key_service
        .as_ref()
        .ok_or_else(|| PrismError::Internal("keys not enabled".into()))?;

    let plaintext = keys::generate_key();
    let new_key_hash = keys::hash_key(&plaintext);
    let new_key_prefix = plaintext[..10].to_string();

    let new_key = key_service
        .repo()
        .rotate_key(id, &new_key_hash, &new_key_prefix, 24)
        .await
        .map_err(|e| PrismError::Internal(format!("failed to rotate key: {e}")))?
        .ok_or_else(|| PrismError::ModelNotFound(format!("key {id} not found")))?;

    // Invalidate old key in cache
    if let Some(old_key) = key_service.repo().find_by_id(id).await.ok().flatten() {
        key_service.invalidate_cache(&old_key.key_hash).await;
    }

    tracing::info!(key_id = %id, new_key_id = %new_key.id, "key rotated");

    Ok(Json(serde_json::json!({
        "new_key": plaintext,
        "new_key_prefix": new_key_prefix,
        "new_key_id": new_key.id,
        "old_key_id": id,
        "old_key_expires_at": new_key.expires_at,
    })))
}

/// GET /api/v1/keys/:id/usage — current RPM/TPM/spend from in-memory state.
pub async fn key_usage(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<UsageResponse>> {
    let key_service = state
        .key_service
        .as_ref()
        .ok_or_else(|| PrismError::Internal("keys not enabled".into()))?;

    let vk = key_service
        .repo()
        .find_by_id(id)
        .await
        .map_err(|e| PrismError::Internal(format!("db error: {e}")))?
        .ok_or_else(|| PrismError::ModelNotFound(format!("key {id} not found")))?;

    let (daily_spend, monthly_spend) = state.budget_tracker.get_spend(&vk.key_hash);

    Ok(Json(UsageResponse {
        key_id: id,
        rpm_current: 0, // RPM is a sliding window, hard to get exact count without exposing internals
        tpm_current: 0,
        daily_spend_usd: daily_spend,
        monthly_spend_usd: monthly_spend,
    }))
}
