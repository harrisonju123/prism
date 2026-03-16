use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;
use crate::routing::policy::{parse_policy_yaml, validate_policy};
use crate::routing::types::RoutingDecision;
use prism_types::PolicyResponse;

/// POST /api/v1/routing/dry-run — simulate a routing decision.
pub async fn dry_run(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DryRunRequest>,
) -> Result<Response> {
    if !state.config.routing.enabled {
        return Err(PrismError::BadRequest("routing is not enabled".into()));
    }

    let task_type = crate::types::TaskType::from_str_loose(&request.task_type);

    let decision = crate::routing::resolve(
        task_type,
        request.confidence.unwrap_or(0.8),
        &request.model,
        &state.fitness_cache,
        &state.routing_policy,
        state.config.routing.tier1_confidence_threshold,
        None,
    )
    .await;

    Ok(Json(DryRunResponse { decision }).into_response())
}

/// POST /api/v1/routing/validate — validate a YAML routing policy.
pub async fn validate(Json(request): Json<ValidateRequest>) -> Result<Response> {
    match parse_policy_yaml(&request.yaml) {
        Ok(policy) => Ok(Json(ValidateResponse {
            valid: true,
            error: None,
            rule_count: policy.rules.len(),
            version: policy.version,
        })
        .into_response()),
        Err(e) => Ok(Json(ValidateResponse {
            valid: false,
            error: Some(e),
            rule_count: 0,
            version: 0,
        })
        .into_response()),
    }
}

/// GET /api/v1/routing/policy — return the current active routing policy.
pub async fn get_policy(State(state): State<Arc<AppState>>) -> Result<Response> {
    let policy = &state.routing_policy;

    // Also run validation to include status
    let validation = validate_policy(policy);

    Ok(Json(PolicyResponse {
        version: policy.version,
        rule_count: policy.rules.len(),
        rules: policy.rules.clone(),
        valid: validation.is_ok(),
    })
    .into_response())
}

#[derive(Debug, Deserialize)]
pub struct DryRunRequest {
    pub model: String,
    pub task_type: String,
    pub confidence: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct DryRunResponse {
    pub decision: Option<RoutingDecision>,
}

#[derive(Debug, Deserialize)]
pub struct ValidateRequest {
    pub yaml: String,
}

#[derive(Debug, Serialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub error: Option<String>,
    pub rule_count: usize,
    pub version: u32,
}
