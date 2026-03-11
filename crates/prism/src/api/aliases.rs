use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};

use crate::error::{PrismError, Result};
use crate::keys::MasterAuth;
use crate::models::alias::ModelAlias;
use crate::proxy::handler::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateAliasRequest {
    pub name: String,
    pub target_model: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAliasRequest {
    #[serde(default)]
    pub target_model: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
}

#[derive(Debug, Serialize)]
pub struct AliasResponse {
    pub name: String,
    pub target_model: String,
    pub description: Option<String>,
}

impl From<ModelAlias> for AliasResponse {
    fn from(a: ModelAlias) -> Self {
        Self {
            name: a.name,
            target_model: a.target_model,
            description: a.description,
        }
    }
}

/// GET /api/v1/aliases
pub async fn list_aliases(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
) -> Result<Json<Vec<AliasResponse>>> {
    let repo = state
        .alias_repo
        .as_ref()
        .ok_or_else(|| PrismError::Internal("aliases not enabled".into()))?;

    let aliases = repo
        .list()
        .await
        .map_err(|e| PrismError::Internal(format!("alias list failed: {e}")))?;

    Ok(Json(aliases.into_iter().map(AliasResponse::from).collect()))
}

/// POST /api/v1/aliases
pub async fn create_alias(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Json(body): Json<CreateAliasRequest>,
) -> Result<Json<AliasResponse>> {
    let repo = state
        .alias_repo
        .as_ref()
        .ok_or_else(|| PrismError::Internal("aliases not enabled".into()))?;

    let alias = repo
        .create(&body.name, &body.target_model, body.description.as_deref())
        .await
        .map_err(|e| PrismError::Internal(format!("alias create failed: {e}")))?;

    // Update cache
    if let Some(ref cache) = state.alias_cache {
        cache
            .set(alias.name.clone(), alias.target_model.clone())
            .await;
    }

    Ok(Json(AliasResponse::from(alias)))
}

/// PUT /api/v1/aliases/:name
pub async fn update_alias(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Path(name): Path<String>,
    Json(body): Json<UpdateAliasRequest>,
) -> Result<Json<AliasResponse>> {
    let repo = state
        .alias_repo
        .as_ref()
        .ok_or_else(|| PrismError::Internal("aliases not enabled".into()))?;

    let desc = body
        .description
        .map(|opt| opt.as_deref().map(|s| s.to_string()));
    let desc_ref: Option<Option<&str>> = desc.as_ref().map(|opt| opt.as_deref());

    let alias = repo
        .update(&name, body.target_model.as_deref(), desc_ref)
        .await
        .map_err(|e| PrismError::Internal(format!("alias update failed: {e}")))?
        .ok_or_else(|| PrismError::ModelNotFound(format!("alias '{name}' not found")))?;

    // Update cache
    if let Some(ref cache) = state.alias_cache {
        cache
            .set(alias.name.clone(), alias.target_model.clone())
            .await;
    }

    Ok(Json(AliasResponse::from(alias)))
}

/// DELETE /api/v1/aliases/:name
pub async fn delete_alias(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let repo = state
        .alias_repo
        .as_ref()
        .ok_or_else(|| PrismError::Internal("aliases not enabled".into()))?;

    let deleted = repo
        .delete(&name)
        .await
        .map_err(|e| PrismError::Internal(format!("alias delete failed: {e}")))?;

    if !deleted {
        return Err(PrismError::ModelNotFound(format!(
            "alias '{name}' not found"
        )));
    }

    // Remove from cache
    if let Some(ref cache) = state.alias_cache {
        cache.remove(&name).await;
    }

    Ok(Json(serde_json::json!({ "deleted": true, "name": name })))
}
