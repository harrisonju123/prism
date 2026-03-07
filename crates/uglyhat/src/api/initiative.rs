use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Error;

#[derive(Deserialize)]
pub struct CreateInitiativeReq {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct UpdateInitiativeReq {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: String,
    pub metadata: Option<serde_json::Value>,
}

pub async fn create_initiative(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateInitiativeReq>,
) -> Result<impl IntoResponse, Error> {
    if req.name.is_empty() {
        return Err(Error::BadRequest("name is required".into()));
    }
    let init = state
        .store
        .create_initiative(workspace_id, &req.name, &req.description, req.metadata)
        .await?;
    Ok((StatusCode::CREATED, Json(init)))
}

pub async fn list_initiatives(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let inits = state
        .store
        .list_initiatives_by_workspace(workspace_id)
        .await?;
    Ok(Json(inits))
}

pub async fn get_initiative(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let init = state.store.get_initiative(id).await?;
    Ok(Json(init))
}

pub async fn update_initiative(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateInitiativeReq>,
) -> Result<impl IntoResponse, Error> {
    let init = state
        .store
        .update_initiative(id, &req.name, &req.description, &req.status, req.metadata)
        .await?;
    Ok(Json(init))
}

pub async fn delete_initiative(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    state.store.delete_initiative(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
