use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::error::{PrismError, Result};
use crate::interop::metering::MeteringSummary;
use crate::interop::types::{AgentCapability, InvocationRequest, ProtocolMessage};
use crate::keys::MasterAuth;
use crate::proxy::handler::AppState;

#[derive(Debug, Deserialize)]
pub struct DiscoverQuery {
    pub method: Option<String>,
    pub framework: Option<String>,
}

/// POST /api/v1/interop/invoke
pub async fn invoke(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Json(request): Json<InvocationRequest>,
) -> Result<Json<ProtocolMessage>> {
    let (_bridge, _metering, secret) = get_interop_components(&state)?;

    let msg =
        crate::interop::protocol::create_invocation(&request, &request.caller_agent_id, secret);

    tracing::info!(
        caller = %request.caller_agent_id,
        target = %request.target_listing_id,
        method = %request.method,
        "interop invocation created"
    );

    Ok(Json(msg))
}

/// POST /api/v1/interop/register
pub async fn register(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Json(capability): Json<AgentCapability>,
) -> Result<Json<serde_json::Value>> {
    let (bridge, _, _) = get_interop_components(&state)?;
    let listing_id = capability.listing_id.clone();
    bridge.register(capability);
    Ok(Json(
        serde_json::json!({ "registered": true, "listing_id": listing_id }),
    ))
}

/// GET /api/v1/interop/discover
pub async fn discover(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Query(params): Query<DiscoverQuery>,
) -> Result<Json<Vec<AgentCapability>>> {
    let (bridge, _, _) = get_interop_components(&state)?;
    let results = bridge.discover(params.method.as_deref(), params.framework.as_deref());
    Ok(Json(results))
}

/// GET /api/v1/interop/metering
pub async fn metering_summary(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
) -> Result<Json<MeteringSummary>> {
    let (_, metering, _) = get_interop_components(&state)?;
    Ok(Json(metering.summary()))
}

fn get_interop_components(
    state: &AppState,
) -> Result<(
    &crate::interop::bridge::DiscoveryBridge,
    &crate::interop::metering::MeteringStore,
    &str,
)> {
    let bridge = state
        .interop_bridge
        .as_ref()
        .ok_or_else(|| PrismError::Internal("interop not enabled".into()))?;
    let metering = state
        .interop_metering
        .as_ref()
        .ok_or_else(|| PrismError::Internal("interop not enabled".into()))?;
    let secret = state
        .config
        .interop
        .hmac_secret
        .as_deref()
        .unwrap_or("default-secret");
    Ok((bridge, metering, secret))
}
