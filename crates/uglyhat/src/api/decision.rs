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
pub struct CreateDecisionReq {
    pub title: String,
    #[serde(default)]
    pub content: String,
    pub workspace_id: Option<Uuid>,
    pub initiative_id: Option<Uuid>,
    pub epic_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct UpdateDecisionReq {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub status: String,
    pub metadata: Option<serde_json::Value>,
}

pub async fn create_decision(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateDecisionReq>,
) -> Result<impl IntoResponse, Error> {
    if req.title.is_empty() {
        return Err(Error::BadRequest("title is required".into()));
    }
    let decision = state
        .store
        .create_decision(
            req.workspace_id,
            req.initiative_id,
            req.epic_id,
            &req.title,
            &req.content,
            req.metadata,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(decision)))
}

pub async fn get_decision(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let decision = state.store.get_decision(id).await?;
    Ok(Json(decision))
}

pub async fn list_decisions(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let decisions = state
        .store
        .list_decisions_by_workspace(workspace_id)
        .await?;
    Ok(Json(decisions))
}

pub async fn update_decision(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateDecisionReq>,
) -> Result<impl IntoResponse, Error> {
    let decision = state
        .store
        .update_decision(id, &req.title, &req.content, &req.status, req.metadata)
        .await?;
    Ok(Json(decision))
}

pub async fn delete_decision(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    state.store.delete_decision(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
