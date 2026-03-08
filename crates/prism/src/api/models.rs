use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::models::MODEL_CATALOG;
use crate::proxy::handler::AppState;

#[derive(Serialize)]
pub struct ModelsResponse {
    object: &'static str,
    data: Vec<ModelEntry>,
}

#[derive(Serialize)]
pub struct ModelEntry {
    id: String,
    object: &'static str,
    owned_by: String,
}

/// GET /v1/models — list available models.
pub async fn list_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut models: Vec<ModelEntry> = Vec::new();

    // Add configured model aliases
    for (alias, config) in &state.config.models {
        models.push(ModelEntry {
            id: alias.clone(),
            object: "model",
            owned_by: config.provider.clone(),
        });
    }

    // Add catalog models not already covered by aliases
    let alias_ids: std::collections::HashSet<&str> = state
        .config
        .models
        .values()
        .map(|m| m.model.as_str())
        .collect();

    for (name, info) in MODEL_CATALOG.iter() {
        if !alias_ids.contains(info.model_id) {
            models.push(ModelEntry {
                id: name.to_string(),
                object: "model",
                owned_by: info.provider.to_string(),
            });
        }
    }

    let cost_micros = state.session_cost_usd.load(Ordering::Relaxed);
    let cost_usd = cost_micros as f64 / 1_000_000.0;

    let mut response = Json(ModelsResponse {
        object: "list",
        data: models,
    })
    .into_response();

    if let Ok(val) = HeaderValue::from_str(&format!("{cost_usd:.6}")) {
        response
            .headers_mut()
            .insert("x-prism-session-cost-usd", val);
    }

    response
}
