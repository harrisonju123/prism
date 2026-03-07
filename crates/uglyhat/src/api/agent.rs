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
pub struct CheckinReq {
    pub name: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Deserialize)]
pub struct CheckoutReq {
    pub name: String,
    #[serde(default)]
    pub summary: String,
}

pub async fn checkin(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CheckinReq>,
) -> Result<impl IntoResponse, Error> {
    if req.name.is_empty() {
        return Err(Error::BadRequest("name is required".into()));
    }
    let resp = state
        .store
        .checkin_agent(workspace_id, &req.name, req.capabilities)
        .await?;
    Ok((StatusCode::OK, Json(resp)))
}

pub async fn checkout(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CheckoutReq>,
) -> Result<impl IntoResponse, Error> {
    if req.name.is_empty() {
        return Err(Error::BadRequest("name is required".into()));
    }
    let session = state
        .store
        .checkout_agent(workspace_id, &req.name, &req.summary)
        .await?;
    Ok(Json(session))
}

pub async fn list_agents(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let agents = state.store.list_agents(workspace_id).await?;
    Ok(Json(agents))
}

pub async fn list_agent_statuses(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let statuses = state.store.list_agent_statuses(workspace_id).await?;
    Ok(Json(statuses))
}
