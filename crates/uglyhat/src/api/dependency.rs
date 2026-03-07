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
pub struct AddDependencyReq {
    pub blocking_task_id: Uuid,
    pub blocked_task_id: Uuid,
}

pub async fn add_dependency(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    Json(req): Json<AddDependencyReq>,
) -> Result<impl IntoResponse, Error> {
    let _ = task_id;
    let dep = state
        .store
        .add_dependency(req.blocking_task_id, req.blocked_task_id)
        .await?;
    Ok((StatusCode::CREATED, Json(dep)))
}

pub async fn remove_dependency(
    State(state): State<Arc<AppState>>,
    Path((_task_id, dep_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, Error> {
    state.store.remove_dependency(dep_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_dependencies(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let (blocks, blocked_by) = state.store.get_dependencies(task_id).await?;
    Ok(Json(serde_json::json!({
        "task_id": task_id,
        "blocks": blocks,
        "blocked_by": blocked_by,
    })))
}
