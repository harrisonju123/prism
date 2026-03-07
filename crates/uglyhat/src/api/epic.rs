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
pub struct CreateEpicReq {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct UpdateEpicReq {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: String,
    pub metadata: Option<serde_json::Value>,
}

pub async fn create_epic(
    State(state): State<Arc<AppState>>,
    Path(initiative_id): Path<Uuid>,
    Json(req): Json<CreateEpicReq>,
) -> Result<impl IntoResponse, Error> {
    if req.name.is_empty() {
        return Err(Error::BadRequest("name is required".into()));
    }
    let epic = state
        .store
        .create_epic(initiative_id, &req.name, &req.description, req.metadata)
        .await?;
    Ok((StatusCode::CREATED, Json(epic)))
}

pub async fn list_epics(
    State(state): State<Arc<AppState>>,
    Path(initiative_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let epics = state.store.list_epics_by_initiative(initiative_id).await?;
    Ok(Json(epics))
}

pub async fn get_epic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let epic = state.store.get_epic(id).await?;
    Ok(Json(epic))
}

pub async fn update_epic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateEpicReq>,
) -> Result<impl IntoResponse, Error> {
    let epic = state
        .store
        .update_epic(id, &req.name, &req.description, &req.status, req.metadata)
        .await?;
    Ok(Json(epic))
}

pub async fn delete_epic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    state.store.delete_epic(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
