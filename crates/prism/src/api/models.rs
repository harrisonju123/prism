use std::sync::Arc;

use axum::Json;
use axum::extract::State;
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
pub async fn list_models(State(state): State<Arc<AppState>>) -> Json<ModelsResponse> {
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

    Json(ModelsResponse {
        object: "list",
        data: models,
    })
}
