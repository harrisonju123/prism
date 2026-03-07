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
pub struct CreateNoteReq {
    pub title: String,
    #[serde(default)]
    pub content: String,
    pub workspace_id: Option<Uuid>,
    pub initiative_id: Option<Uuid>,
    pub epic_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub decision_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct UpdateNoteReq {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    pub metadata: Option<serde_json::Value>,
}

pub async fn create_note(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateNoteReq>,
) -> Result<impl IntoResponse, Error> {
    if req.title.is_empty() {
        return Err(Error::BadRequest("title is required".into()));
    }
    let note = state
        .store
        .create_note(
            req.workspace_id,
            req.initiative_id,
            req.epic_id,
            req.task_id,
            req.decision_id,
            &req.title,
            &req.content,
            req.metadata,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(note)))
}

pub async fn get_note(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let note = state.store.get_note(id).await?;
    Ok(Json(note))
}

pub async fn list_notes_by_parent(
    State(state): State<Arc<AppState>>,
    Path((parent_type, parent_id)): Path<(String, Uuid)>,
) -> Result<impl IntoResponse, Error> {
    let notes = state
        .store
        .list_notes_by_parent(&parent_type, parent_id)
        .await?;
    Ok(Json(notes))
}

pub async fn update_note(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateNoteReq>,
) -> Result<impl IntoResponse, Error> {
    let note = state
        .store
        .update_note(id, &req.title, &req.content, req.metadata)
        .await?;
    Ok(Json(note))
}

pub async fn delete_note(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    state.store.delete_note(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
