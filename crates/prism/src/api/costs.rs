use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;
use prism_types::ThreadCostResponse;

#[derive(Debug, Deserialize)]
pub struct CostParams {
    pub thread_id: String,
    #[serde(default = "default_period_days")]
    pub period_days: u32,
}

fn default_period_days() -> u32 {
    30
}

/// GET /v1/costs?thread_id=X
///
/// Aggregated cost for a context thread from the daily_spend_by_thread materialized view.
pub async fn thread_costs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CostParams>,
) -> Result<Response> {
    let client = &state.http_client;
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;

    // Sanitize thread_id: only allow alphanumeric, dash, underscore
    let sanitized: String = params
        .thread_id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();

    if sanitized.is_empty() {
        return Err(PrismError::BadRequest("thread_id is required".into()));
    }

    let query = format!(
        "SELECT \
            thread_id, \
            sum(request_count) AS request_count, \
            sum(total_cost_usd) AS total_cost_usd, \
            sum(total_input_tokens) AS total_input_tokens, \
            sum(total_output_tokens) AS total_output_tokens \
         FROM {db}.daily_spend_by_thread \
         WHERE day >= now() - INTERVAL {days} DAY \
           AND thread_id = '{thread_id}' \
         GROUP BY thread_id \
         FORMAT JSONEachRow",
        db = ch_db,
        days = params.period_days,
        thread_id = sanitized,
    );

    let resp = client
        .post(ch_url)
        .body(query)
        .send()
        .await
        .map_err(|e| PrismError::Internal(format!("clickhouse query failed: {e}")))?
        .text()
        .await
        .map_err(|e| PrismError::Internal(format!("clickhouse response error: {e}")))?;

    // Parse first line (single thread_id aggregation)
    if let Some(line) = resp.lines().next() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            return Ok(Json(ThreadCostResponse {
                thread_id: sanitized,
                total_cost_usd: v
                    .get("total_cost_usd")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                request_count: v.get("request_count").and_then(|v| v.as_u64()).unwrap_or(0),
                total_input_tokens: v
                    .get("total_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_output_tokens: v
                    .get("total_output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            })
            .into_response());
        }
    }

    // No data found — return zeroes
    Ok(Json(ThreadCostResponse {
        thread_id: sanitized,
        total_cost_usd: 0.0,
        request_count: 0,
        total_input_tokens: 0,
        total_output_tokens: 0,
    })
    .into_response())
}
