use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;

#[derive(Debug, Deserialize)]
pub struct WasteReportParams {
    #[serde(default = "default_period_days")]
    pub period_days: u32,
}

fn default_period_days() -> u32 {
    7
}

/// GET /api/v1/waste-report
pub async fn waste_report(
    State(state): State<Arc<AppState>>,
    Query(params): Query<WasteReportParams>,
) -> Result<Response> {
    if !state.config.waste.enabled {
        return Err(PrismError::BadRequest(
            "waste detection is disabled".to_string(),
        ));
    }

    let report = super::detector::generate_waste_report(
        &state.config.clickhouse.url,
        &state.config.clickhouse.database,
        &state.fitness_cache,
        &state.config.waste,
        params.period_days,
    )
    .await
    .map_err(|e| PrismError::Internal(format!("waste report generation failed: {e}")))?;

    Ok(Json(report).into_response())
}
