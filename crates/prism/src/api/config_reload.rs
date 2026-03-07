use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::config::Config;
use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;
use crate::routing::policy::load_policy;

/// POST /api/v1/config/reload — reload configuration from disk.
pub async fn reload_config(State(state): State<Arc<AppState>>) -> Result<Response> {
    let new_config = Config::load(None)
        .map_err(|e| PrismError::Internal(format!("failed to load config: {e}")))?;

    // Swap the hot-reloadable config
    if let Some(ref swappable) = state.hot_config {
        let _old_version = swappable.load();

        // Rebuild routing policy from new config
        let new_policy = load_policy(new_config.routing.rules.clone());

        // Store new config
        swappable.store(Arc::new(new_config.clone()));

        // Update routing policy if it changed
        if let Some(ref swappable_policy) = state.hot_routing_policy {
            swappable_policy.store(Arc::new(new_policy));
        }

        tracing::info!("configuration reloaded successfully");

        Ok(Json(ReloadResponse {
            success: true,
            message: "configuration reloaded".into(),
        })
        .into_response())
    } else {
        Err(PrismError::Internal(
            "hot reload not enabled (hot_config not initialized)".into(),
        ))
    }
}

/// GET /api/v1/config — return current configuration (redacted).
pub async fn get_config(State(state): State<Arc<AppState>>) -> Result<Response> {
    let config = if let Some(ref hot) = state.hot_config {
        (**hot.load()).clone()
    } else {
        state.config.clone()
    };

    Ok(Json(ConfigSummary {
        gateway_address: config.gateway.address.clone(),
        routing_enabled: config.routing.enabled,
        cache_enabled: config.cache.enabled,
        keys_enabled: config.keys.enabled,
        benchmark_enabled: config.benchmark.enabled,
        alerts_enabled: config.alerts.enabled,
        provider_count: config.providers.len(),
        model_count: config.models.len(),
    })
    .into_response())
}

#[derive(Debug, Serialize)]
pub struct ReloadResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ConfigSummary {
    pub gateway_address: String,
    pub routing_enabled: bool,
    pub cache_enabled: bool,
    pub keys_enabled: bool,
    pub benchmark_enabled: bool,
    pub alerts_enabled: bool,
    pub provider_count: usize,
    pub model_count: usize,
}
