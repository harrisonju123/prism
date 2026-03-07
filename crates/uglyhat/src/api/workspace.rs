use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Error;
use crate::middleware::auth::WorkspaceId;
use crate::model::{APIKeyWithRaw, BootstrapResponse, TaskPriority, TaskStatus};

#[derive(Deserialize)]
pub struct CreateWorkspaceReq {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub metadata: Option<serde_json::Value>,
}

pub async fn create_workspace(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateWorkspaceReq>,
) -> Result<impl IntoResponse, Error> {
    if req.name.is_empty() {
        return Err(Error::BadRequest("name is required".into()));
    }

    let (raw_key, key_hash, key_prefix) = super::apikey::generate_api_key();

    let result = state
        .store
        .bootstrap_workspace(&req.name, &req.description, &key_hash, &key_prefix)
        .await?;

    let resp = BootstrapResponse {
        workspace: result.workspace,
        system_initiative_id: result.initiative_id,
        system_epic_id: result.epic_id,
        api_key: APIKeyWithRaw {
            api_key: result.api_key,
            key: raw_key,
        },
    };

    Ok((StatusCode::CREATED, Json(resp)))
}

pub async fn list_workspaces(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, Error> {
    let ws = state.store.list_workspaces().await?;
    Ok(Json(ws))
}

pub async fn get_workspace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let ws = state.store.get_workspace(id).await?;
    Ok(Json(ws))
}

pub async fn update_workspace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateWorkspaceReq>,
) -> Result<impl IntoResponse, Error> {
    let ws = state
        .store
        .update_workspace(id, &req.name, &req.description, req.metadata)
        .await?;
    Ok(Json(ws))
}

pub async fn delete_workspace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    state.store.delete_workspace(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ReportIssueReq {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub domain_tags: Vec<String>,
    pub metadata: Option<serde_json::Value>,
}

pub async fn report_issue(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Extension(WorkspaceId(_ws_id)): Extension<WorkspaceId>,
    Json(req): Json<ReportIssueReq>,
) -> Result<impl IntoResponse, Error> {
    if req.title.is_empty() {
        return Err(Error::BadRequest("title is required".into()));
    }

    let priority = if req.severity.is_empty() {
        TaskPriority::Medium
    } else {
        serde_json::from_value::<TaskPriority>(serde_json::Value::String(req.severity.clone()))
            .map_err(|_| Error::BadRequest(format!("invalid severity: {}", req.severity)))?
    };

    let epic_id = state.store.get_system_epic_id(workspace_id).await?;

    let mut tags = req.domain_tags;
    tags.push("agent-reported".to_string());

    let mut meta = req
        .metadata
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    meta.insert(
        "reported_by".to_string(),
        serde_json::Value::String(req.source.clone()),
    );
    meta.insert(
        "issue_type".to_string(),
        serde_json::Value::String("agent-reported".to_string()),
    );

    let task = state
        .store
        .create_task(
            epic_id,
            &req.title,
            &req.description,
            TaskStatus::Backlog,
            priority,
            &req.source,
            tags,
            Some(serde_json::Value::Object(meta)),
        )
        .await?;

    Ok((StatusCode::CREATED, Json(task)))
}
