use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Error;
use crate::store::HandoffFilters;

#[derive(Deserialize)]
pub struct CreateHandoffReq {
    pub task_id: Uuid,
    #[serde(default)]
    pub agent_name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub next_steps: Vec<String>,
    pub artifacts: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct HandoffQuery {
    pub since: Option<String>,
    pub agent: Option<String>,
}

pub async fn create_handoff(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateHandoffReq>,
) -> Result<impl IntoResponse, Error> {
    let _ = workspace_id;
    let handoff = state
        .store
        .create_handoff(
            req.task_id,
            &req.agent_name,
            &req.summary,
            req.findings,
            req.blockers,
            req.next_steps,
            req.artifacts,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(handoff)))
}

pub async fn list_handoffs(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Query(q): Query<HandoffQuery>,
) -> Result<impl IntoResponse, Error> {
    let since = q
        .since
        .map(|s| crate::api::parse_rfc3339_param(&s))
        .transpose()?;

    let filters = HandoffFilters {
        since,
        agent: q.agent,
    };
    let handoffs = state
        .store
        .list_handoffs_by_workspace(workspace_id, filters)
        .await?;
    Ok(Json(handoffs))
}

pub async fn get_handoffs_by_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let handoffs = state.store.get_handoffs_by_task(task_id).await?;
    Ok(Json(handoffs))
}
