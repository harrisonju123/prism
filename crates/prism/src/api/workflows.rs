use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};

use crate::error::Result;
use crate::proxy::handler::AppState;
use crate::workflows::engine::execute_workflow;
use crate::workflows::types::ExecuteWorkflowRequest;

/// POST /api/v1/workflows/execute — execute a workflow DAG.
pub async fn execute(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ExecuteWorkflowRequest>,
) -> Result<Response> {
    let result = execute_workflow(&request, &state.config, &state.providers)
        .await
        .map_err(|e| crate::error::PrismError::BadRequest(e))?;

    Ok(Json(result).into_response())
}
