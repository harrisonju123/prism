use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::error::{PrismError, Result};
use crate::prompts::store::PromptStore;
use crate::prompts::types::{CreatePromptRequest, PromptListResponse};
use crate::proxy::handler::AppState;

fn get_store(state: &AppState) -> Result<&PromptStore> {
    state
        .prompt_store
        .as_deref()
        .ok_or_else(|| PrismError::Internal("prompt store not initialized".into()))
}

/// POST /api/v1/prompts — create a new prompt template.
pub async fn create_prompt(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreatePromptRequest>,
) -> Result<Response> {
    let store = get_store(&state)?;

    let template = store
        .create_async(
            &request.name,
            &request.content,
            request.model_hint,
            request.metadata,
        )
        .await
        .map_err(|e| PrismError::Internal(e))?;

    Ok(Json(template).into_response())
}

/// GET /api/v1/prompts — list all prompt templates.
pub async fn list_prompts(State(state): State<Arc<AppState>>) -> Result<Response> {
    let store = get_store(&state)?;
    let prompts = store.list_async().await;
    Ok(Json(PromptListResponse { prompts }).into_response())
}

#[derive(Debug, Deserialize)]
pub struct VersionQuery {
    pub version: Option<u32>,
}

/// GET /api/v1/prompts/:name — get a prompt by name (optional ?version=N).
pub async fn get_prompt(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(params): Query<VersionQuery>,
) -> Result<Response> {
    let store = get_store(&state)?;

    let template = if let Some(version) = params.version {
        store.get_version_async(&name, version).await
    } else {
        store.get_latest_async(&name).await
    };

    match template {
        Some(t) => Ok(Json(t).into_response()),
        None => Err(PrismError::ModelNotFound(format!(
            "prompt '{}' not found",
            name
        ))),
    }
}

/// GET /api/v1/prompts/:name/versions — get version history.
pub async fn get_versions(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Response> {
    let store = get_store(&state)?;
    let versions = store.get_versions_async(&name).await;
    Ok(Json(PromptListResponse { prompts: versions }).into_response())
}

/// POST /api/v1/prompts/:name/rollback/:version — rollback to a specific version.
pub async fn rollback_prompt(
    State(state): State<Arc<AppState>>,
    Path((name, version)): Path<(String, u32)>,
) -> Result<Response> {
    let store = get_store(&state)?;
    let template = store
        .rollback_async(&name, version)
        .await
        .map_err(|e| PrismError::Internal(e))?;
    Ok(Json(template).into_response())
}
