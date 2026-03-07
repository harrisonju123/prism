use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Error;
use crate::model::{TaskPriority, TaskStatus};
use crate::store::TaskFilters;

#[derive(Deserialize)]
pub struct CreateTaskReq {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: Option<TaskStatus>,
    #[serde(default)]
    pub priority: Option<TaskPriority>,
    #[serde(default)]
    pub assignee: String,
    #[serde(default)]
    pub domain_tags: Vec<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct UpdateTaskReq {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    #[serde(default)]
    pub assignee: String,
    #[serde(default)]
    pub domain_tags: Vec<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct TaskListQuery {
    pub status: Option<String>,
    pub priority: Option<String>,
    pub domain: Option<String>,
    pub assignee: Option<String>,
    pub unassigned: Option<bool>,
}

pub async fn create_task(
    State(state): State<Arc<AppState>>,
    Path(epic_id): Path<Uuid>,
    Json(req): Json<CreateTaskReq>,
) -> Result<impl IntoResponse, Error> {
    if req.name.is_empty() {
        return Err(Error::BadRequest("name is required".into()));
    }
    let task = state
        .store
        .create_task(
            epic_id,
            &req.name,
            &req.description,
            req.status.unwrap_or(TaskStatus::Backlog),
            req.priority.unwrap_or(TaskPriority::Medium),
            &req.assignee,
            req.domain_tags,
            req.metadata,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(task)))
}

pub async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let task = state.store.get_task(id).await?;
    Ok(Json(task))
}

pub async fn update_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateTaskReq>,
) -> Result<impl IntoResponse, Error> {
    if req.name.is_empty() {
        return Err(Error::BadRequest("name is required".into()));
    }
    let task = state
        .store
        .update_task(
            id,
            &req.name,
            &req.description,
            req.status,
            req.priority,
            &req.assignee,
            req.domain_tags,
            req.metadata,
        )
        .await?;
    Ok(Json(task))
}

pub async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    state.store.delete_task(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_tasks_by_epic(
    State(state): State<Arc<AppState>>,
    Path(epic_id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let tasks = state.store.list_tasks_by_epic(epic_id).await?;
    Ok(Json(tasks))
}

pub async fn list_tasks_by_workspace(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<Uuid>,
    Query(q): Query<TaskListQuery>,
) -> Result<impl IntoResponse, Error> {
    let status = q
        .status
        .map(|s| {
            serde_json::from_value::<TaskStatus>(serde_json::Value::String(s.clone()))
                .map_err(|_| Error::BadRequest(format!("invalid status: {s}")))
        })
        .transpose()?;
    let priority = q
        .priority
        .map(|p| {
            serde_json::from_value::<TaskPriority>(serde_json::Value::String(p.clone()))
                .map_err(|_| Error::BadRequest(format!("invalid priority: {p}")))
        })
        .transpose()?;

    let filters = TaskFilters {
        status,
        priority,
        domain: q.domain,
        assignee: q.assignee,
        unassigned: q.unassigned,
    };
    let tasks = state
        .store
        .list_tasks_by_workspace(workspace_id, filters)
        .await?;
    Ok(Json(tasks))
}

pub async fn get_task_context(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let ctx = state.store.get_task_context(id).await?;
    Ok(Json(ctx))
}
