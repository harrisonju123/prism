use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Error;
use crate::store::ActivityFilters;

#[derive(Deserialize)]
pub struct ActivityQuery {
    pub since: Option<String>,
    pub actor: Option<String>,
    pub entity_type: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    50
}

pub async fn list_activity(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Query(q): Query<ActivityQuery>,
) -> Result<impl IntoResponse, Error> {
    let since = q.since.map(|s| crate::api::parse_rfc3339_param(&s)).transpose()?;

    let filters = ActivityFilters {
        since,
        actor: q.actor,
        entity_type: q.entity_type,
        limit: q.limit,
    };
    let entries = state.store.list_activity(workspace_id, filters).await?;
    Ok(Json(entries))
}

#[derive(Deserialize)]
pub struct CreateActivityReq {
    #[serde(default)]
    pub actor: String,
    pub action: String,
    pub entity_type: String,
    pub entity_id: Uuid,
    #[serde(default)]
    pub summary: String,
    pub detail: Option<serde_json::Value>,
}

pub async fn create_activity(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateActivityReq>,
) -> Result<impl IntoResponse, Error> {
    let entry = state
        .store
        .create_activity(
            workspace_id,
            &req.actor,
            &req.action,
            &req.entity_type,
            req.entity_id,
            &req.summary,
            req.detail,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(entry)))
}
