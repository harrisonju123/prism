use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Error;

#[derive(Deserialize)]
pub struct NextTasksQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    5
}

pub async fn get_workspace_context(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let ctx = state.store.get_workspace_context(workspace_id).await?;
    Ok(Json(ctx))
}

pub async fn get_next_tasks(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Query(q): Query<NextTasksQuery>,
) -> Result<impl IntoResponse, Error> {
    let tasks = state.store.get_next_tasks(workspace_id, q.limit).await?;
    Ok(Json(tasks))
}

pub async fn get_stale_tasks(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let tasks = state.store.get_stale_tasks(workspace_id).await?;
    Ok(Json(tasks))
}
