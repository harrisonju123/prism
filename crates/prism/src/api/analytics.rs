use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::error::{PrismError, Result};
use crate::models::MODEL_CATALOG;
use crate::proxy::handler::AppState;
use prism_types::{
    QualityTrendPoint, QualityTrendsResponse, RoutingSavingsResponse, SessionEfficiencyResponse,
    SessionEfficiencyStat,
};

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct QualityTrendsParams {
    #[serde(default = "default_days")]
    pub since: u32,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RoutingSavingsParams {
    #[serde(default = "default_days")]
    pub since: u32,
}

#[derive(Debug, Deserialize)]
pub struct SessionEfficiencyParams {
    #[serde(default = "default_days")]
    pub since: u32,
}

fn default_days() -> u32 {
    7
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/analytics/quality-trends
pub async fn quality_trends(
    State(state): State<Arc<AppState>>,
    Query(params): Query<QualityTrendsParams>,
) -> Result<Response> {
    let client = &state.http_client;
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;

    let model_filter = params.model.as_deref().map(|m| {
        let sanitized: String = m.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '.' || *c == '_' || *c == ':' || *c == '/').collect();
        format!("AND ie.model = '{sanitized}'")
    }).unwrap_or_default();

    let query = format!(
        "SELECT toString(toStartOfDay(fe.timestamp)) AS day, \
                ie.model, ie.task_type, \
                avg(fe.metric_value) AS avg_quality, \
                count() AS sample_count \
         FROM {db}.feedback_events fe \
         JOIN {db}.inference_events ie ON fe.episode_id = ie.episode_id \
         WHERE fe.timestamp >= now() - INTERVAL {days} DAY \
           {model_filter} \
         GROUP BY day, ie.model, ie.task_type \
         ORDER BY day ASC \
         FORMAT JSONEachRow",
        db = ch_db,
        days = params.since,
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

    let mut data = Vec::new();
    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            data.push(QualityTrendPoint {
                day: v.get("day").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                model: v.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                task_type: v.get("task_type").and_then(|v| v.as_str()).map(String::from),
                avg_quality: v.get("avg_quality").and_then(|v| v.as_f64()).unwrap_or(0.0),
                sample_count: v.get("sample_count").and_then(|v| v.as_u64()).unwrap_or(0),
            });
        }
    }

    Ok(Json(QualityTrendsResponse { data }).into_response())
}

/// GET /api/v1/analytics/routing-savings
pub async fn routing_savings(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RoutingSavingsParams>,
) -> Result<Response> {
    let client = &state.http_client;
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;

    let query = format!(
        "SELECT model, \
                routing_decision, \
                sum(estimated_cost_usd) AS actual_cost, \
                sum(input_tokens) AS total_input, \
                sum(output_tokens) AS total_output, \
                count() AS request_count \
         FROM {db}.inference_events \
         WHERE routing_decision IS NOT NULL \
           AND routing_decision != '' \
           AND timestamp >= now() - INTERVAL {days} DAY \
         GROUP BY model, routing_decision \
         FORMAT JSONEachRow",
        db = ch_db,
        days = params.since,
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

    let mut actual_cost = 0.0;
    let mut counterfactual_cost = 0.0;
    let mut routed_requests: u64 = 0;

    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let cost = v.get("actual_cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let input = v.get("total_input").and_then(|v| v.as_u64()).unwrap_or(0);
            let output = v.get("total_output").and_then(|v| v.as_u64()).unwrap_or(0);
            let count = v.get("request_count").and_then(|v| v.as_u64()).unwrap_or(0);

            actual_cost += cost;
            routed_requests += count;

            // Compute counterfactual: what it would have cost with the originally requested model
            if let Some(rd_str) = v.get("routing_decision").and_then(|v| v.as_str()) {
                if let Ok(rd) = serde_json::from_str::<serde_json::Value>(rd_str) {
                    if let Some(requested_model) = rd.get("requested_model").and_then(|v| v.as_str()) {
                        if let Some(info) = MODEL_CATALOG.get(requested_model) {
                            let cf = info.input_cost_per_token() * input as f64
                                + info.output_cost_per_token() * output as f64;
                            counterfactual_cost += cf;
                        } else {
                            counterfactual_cost += cost;
                        }
                    } else {
                        counterfactual_cost += cost;
                    }
                } else {
                    counterfactual_cost += cost;
                }
            } else {
                counterfactual_cost += cost;
            }
        }
    }

    Ok(Json(RoutingSavingsResponse {
        actual_cost,
        counterfactual_cost,
        savings: counterfactual_cost - actual_cost,
        routed_requests,
    }).into_response())
}

/// GET /api/v1/analytics/session-efficiency
pub async fn session_efficiency(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SessionEfficiencyParams>,
) -> Result<Response> {
    let client = &state.http_client;
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;

    let query = format!(
        "SELECT task_type, \
                avg(turn_count) AS avg_turns, \
                avg(total_cost) AS avg_cost, \
                count() AS session_count \
         FROM ( \
             SELECT episode_id, \
                    anyLast(task_type) AS task_type, \
                    count() AS turn_count, \
                    sum(estimated_cost_usd) AS total_cost \
             FROM {db}.inference_events \
             WHERE episode_id IS NOT NULL \
               AND timestamp >= now() - INTERVAL {days} DAY \
             GROUP BY episode_id \
         ) \
         WHERE task_type IS NOT NULL \
         GROUP BY task_type \
         ORDER BY session_count DESC \
         FORMAT JSONEachRow",
        db = ch_db,
        days = params.since,
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

    let mut data = Vec::new();
    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            data.push(SessionEfficiencyStat {
                task_type: v.get("task_type").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                avg_turns: v.get("avg_turns").and_then(|v| v.as_f64()).unwrap_or(0.0),
                avg_cost: v.get("avg_cost").and_then(|v| v.as_f64()).unwrap_or(0.0),
                session_count: v.get("session_count").and_then(|v| v.as_u64()).unwrap_or(0),
            });
        }
    }

    Ok(Json(SessionEfficiencyResponse { data }).into_response())
}
