use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Error;
use crate::middleware::auth::hash_key;
use crate::model::APIKeyWithRaw;

/// Returns `(raw_key, key_hash, key_prefix)`.
pub(crate) fn generate_api_key() -> (String, String, String) {
    let raw = format!(
        "uh_{}{}",
        hex::encode(Uuid::new_v4().as_bytes()),
        hex::encode(Uuid::new_v4().as_bytes()),
    );
    let hash = hash_key(&raw);
    let prefix = raw[..10].to_string();
    (raw, hash, prefix)
}

#[derive(Deserialize)]
pub struct CreateApiKeyReq {
    pub name: String,
}

pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateApiKeyReq>,
) -> Result<impl IntoResponse, Error> {
    if req.name.is_empty() {
        return Err(Error::BadRequest("name is required".into()));
    }

    let (raw_key, key_hash, key_prefix) = generate_api_key();

    let api_key = state
        .store
        .create_api_key(workspace_id, &req.name, &key_hash, &key_prefix)
        .await?;

    let resp = APIKeyWithRaw {
        api_key,
        key: raw_key,
    };
    Ok((StatusCode::CREATED, Json(resp)))
}

pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let keys = state.store.list_api_keys_by_workspace(workspace_id).await?;
    Ok(Json(keys))
}

pub async fn delete_api_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    state.store.delete_api_key(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
