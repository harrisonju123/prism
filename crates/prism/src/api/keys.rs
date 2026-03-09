use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{PrismError, Result};
use crate::keys::audit::AuditEventType;
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
    #[serde(default)]
    pub rotation_interval_days: Option<i32>,
    #[serde(default)]
    pub allowed_ips: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_origins: Option<Vec<String>>,
}

fn default_budget_action() -> String {
    "reject".into()
}

fn default_metadata() -> serde_json::Value {
    serde_json::json!({})
}

fn serialize_list(v: Vec<String>) -> String {
    serde_json::to_string(&v).unwrap_or_else(|_| "[]".into())
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
    pub rotation_interval_days: Option<i32>,
    pub last_rotated_at: Option<DateTime<Utc>>,
    pub allowed_ips: Option<Vec<String>>,
    pub allowed_origins: Option<Vec<String>>,
}

impl From<VirtualKey> for KeyDetails {
    fn from(vk: VirtualKey) -> Self {
        let allowed_ips: Option<Vec<String>> = vk
            .allowed_ips
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let allowed_origins: Option<Vec<String>> = vk
            .allowed_origins
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
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
            rotation_interval_days: vk.rotation_interval_days,
            last_rotated_at: vk.last_rotated_at,
            allowed_ips,
            allowed_origins,
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
    #[serde(default)]
    pub allowed_ips: Option<Option<Vec<String>>>,
    #[serde(default)]
    pub allowed_origins: Option<Option<Vec<String>>>,
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

    let allowed_ips = body.allowed_ips.map(serialize_list);
    let allowed_origins = body.allowed_origins.map(serialize_list);

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
        rotation_interval_days: body.rotation_interval_days,
        allowed_ips,
        allowed_origins,
    };

    let vk = key_service
        .repo()
        .create(params)
        .await
        .map_err(|e| PrismError::Internal(format!("failed to create key: {e}")))?;

    if let Some(ref audit) = state.audit_service {
        audit.log(
            AuditEventType::KeyCreated,
            Some(vk.id),
            Some(vk.key_prefix.clone()),
            None,
            serde_json::json!({ "name": vk.name }),
            None,
        );
    }

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
    if let Some(ref vk) = vk {
        key_service.invalidate_cache(&vk.key_hash).await;
    }

    if let Some(ref audit) = state.audit_service {
        let (key_prefix, key_id) = vk
            .as_ref()
            .map(|vk| (Some(vk.key_prefix.clone()), Some(vk.id)))
            .unwrap_or((None, None));
        audit.log(
            AuditEventType::KeyRevoked,
            key_id,
            key_prefix,
            None,
            serde_json::json!({ "id": id }),
            None,
        );
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

    let allowed_ips = body.allowed_ips.map(|opt| opt.map(serialize_list));
    let allowed_origins = body.allowed_origins.map(|opt| opt.map(serialize_list));

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
        allowed_ips,
        allowed_origins,
    };

    let vk = key_service
        .repo()
        .update(id, params)
        .await
        .map_err(|e| PrismError::Internal(format!("failed to update key: {e}")))?
        .ok_or_else(|| PrismError::ModelNotFound(format!("key {id} not found")))?;

    // Invalidate cache so new limits take effect
    key_service.invalidate_cache(&vk.key_hash).await;

    if let Some(ref audit) = state.audit_service {
        audit.log(
            AuditEventType::KeyUpdated,
            Some(vk.id),
            Some(vk.key_prefix.clone()),
            None,
            serde_json::json!({ "id": id }),
            None,
        );
    }

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

    if let Some(ref audit) = state.audit_service {
        audit.log(
            AuditEventType::KeyRotated,
            Some(id),
            None,
            None,
            serde_json::json!({ "old_key_id": id, "new_key_id": new_key.id, "new_key_prefix": new_key_prefix }),
            None,
        );
    }

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
    let rpm_current = state.rate_limiter.current_rpm(&vk.key_hash);
    let tpm_current = state.rate_limiter.current_tpm(&vk.key_hash);

    Ok(Json(UsageResponse {
        key_id: id,
        rpm_current,
        tpm_current,
        daily_spend_usd: daily_spend,
        monthly_spend_usd: monthly_spend,
    }))
}
