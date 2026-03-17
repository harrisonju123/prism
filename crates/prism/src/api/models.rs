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
    #[serde(skip_serializing_if = "Option::is_none")]
    prism_input_cost_per_1m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prism_output_cost_per_1m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prism_tier: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prism_context_window: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prism_supports_vision: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prism_supports_tools: Option<bool>,
}

/// GET /v1/models — list available models.
pub async fn list_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut models: Vec<ModelEntry> = Vec::new();

    // Add configured model aliases; pricing comes from catalog if the underlying model is known
    for (alias, config) in &state.config.models {
        let pricing = MODEL_CATALOG.get(config.model.as_str());
        models.push(ModelEntry {
            id: alias.clone(),
            object: "model",
            owned_by: config.provider.clone(),
            prism_input_cost_per_1m: pricing.map(|p| p.input_cost_per_1m),
            prism_output_cost_per_1m: pricing.map(|p| p.output_cost_per_1m),
            prism_tier: pricing.map(|p| p.tier),
            prism_context_window: pricing.map(|p| p.context_window),
            prism_supports_vision: pricing.map(|p| p.supports_vision),
            prism_supports_tools: pricing.map(|p| p.supports_tools),
        });
    }

    // Add catalog models not already covered by aliases
    let alias_ids: std::collections::HashSet<&str> = state
        .config
        .models
        .values()
        .map(|m| m.model.as_str())
        .collect();

    let configured_providers: std::collections::HashSet<&str> =
        state.providers.list().into_iter().collect();

    for (name, info) in MODEL_CATALOG.iter() {
        if !alias_ids.contains(info.model_id) && configured_providers.contains(info.provider) {
            models.push(ModelEntry {
                id: name.to_string(),
                object: "model",
                owned_by: info.provider.to_string(),
                prism_input_cost_per_1m: Some(info.input_cost_per_1m),
                prism_output_cost_per_1m: Some(info.output_cost_per_1m),
                prism_tier: Some(info.tier),
                prism_context_window: Some(info.context_window),
                prism_supports_vision: Some(info.supports_vision),
                prism_supports_tools: Some(info.supports_tools),
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
