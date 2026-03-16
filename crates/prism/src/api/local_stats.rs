use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;

#[derive(Debug, Deserialize)]
pub struct PeriodParams {
    /// Number of days to look back (default: 7)
    #[serde(default = "default_days")]
    pub days: u32,
}

fn default_days() -> u32 {
    7
}

fn pool(state: &AppState) -> Result<&sqlx::SqlitePool> {
    state
        .local_inference_writer
        .as_ref()
        .map(|w| w.pool())
        .ok_or_else(|| {
            PrismError::BadRequest(
                "local observability store is not available in this gateway mode".to_string(),
            )
        })
}

// ---------------------------------------------------------------------------
// GET /api/v1/stats/summary
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct StatsSummary {
    pub period_days: u32,
    pub total_requests: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub error_count: u64,
    pub error_rate: f64,
    pub cache_hits: u64,
    pub cache_hit_rate: f64,
    pub avg_latency_ms: f64,
}

pub async fn stats_summary(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PeriodParams>,
) -> Result<Response> {
    let pool = pool(&state)?;
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(params.days as i64)).to_rfc3339();

    let row = sqlx::query_as::<_, (i64, i64, f64, i64, i64, f64)>(
        "SELECT COUNT(*) as total_requests, \
                COALESCE(SUM(input_tokens + output_tokens), 0) as total_tokens, \
                COALESCE(SUM(estimated_cost_usd), 0.0) as total_cost, \
                SUM(CASE WHEN status = 'Failure' THEN 1 ELSE 0 END) as error_count, \
                SUM(CASE WHEN cache_read_input_tokens > 0 THEN 1 ELSE 0 END) as cache_hits, \
                COALESCE(AVG(latency_ms), 0.0) as avg_latency_ms \
         FROM inference_events WHERE timestamp >= ?",
    )
    .bind(&cutoff)
    .fetch_one(pool)
    .await
    .map_err(|e| PrismError::Internal(e.to_string()))?;

    let total_requests = row.0 as u64;
    let error_count = row.3 as u64;
    let cache_hits = row.4 as u64;

    Ok(Json(StatsSummary {
        period_days: params.days,
        total_requests,
        total_tokens: row.1 as u64,
        total_cost_usd: row.2,
        error_count,
        error_rate: if total_requests > 0 {
            error_count as f64 / total_requests as f64
        } else {
            0.0
        },
        cache_hits,
        cache_hit_rate: if total_requests > 0 {
            cache_hits as f64 / total_requests as f64
        } else {
            0.0
        },
        avg_latency_ms: row.5,
    })
    .into_response())
}

// ---------------------------------------------------------------------------
// GET /api/v1/stats/by-model
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ModelStats {
    pub model: String,
    pub request_count: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub error_count: u64,
    pub avg_latency_ms: f64,
}

pub async fn stats_by_model(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PeriodParams>,
) -> Result<Response> {
    let pool = pool(&state)?;
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(params.days as i64)).to_rfc3339();

    let rows = sqlx::query_as::<_, (String, i64, i64, f64, i64, f64)>(
        "SELECT model, \
                COUNT(*) as request_count, \
                COALESCE(SUM(input_tokens + output_tokens), 0) as total_tokens, \
                COALESCE(SUM(estimated_cost_usd), 0.0) as total_cost, \
                SUM(CASE WHEN status = 'Failure' THEN 1 ELSE 0 END) as error_count, \
                COALESCE(AVG(latency_ms), 0.0) as avg_latency_ms \
         FROM inference_events WHERE timestamp >= ? \
         GROUP BY model \
         ORDER BY total_cost DESC",
    )
    .bind(&cutoff)
    .fetch_all(pool)
    .await
    .map_err(|e| PrismError::Internal(e.to_string()))?;

    let result: Vec<ModelStats> = rows
        .into_iter()
        .map(|(model, req, tokens, cost, errors, lat)| ModelStats {
            model,
            request_count: req as u64,
            total_tokens: tokens as u64,
            total_cost_usd: cost,
            error_count: errors as u64,
            avg_latency_ms: lat,
        })
        .collect();

    Ok(Json(result).into_response())
}

// ---------------------------------------------------------------------------
// GET /api/v1/stats/timeseries
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct TimeseriesBucket {
    /// ISO-8601 hour bucket: "2026-03-16T14:00:00Z"
    pub hour: String,
    pub request_count: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub error_count: u64,
}

pub async fn stats_timeseries(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PeriodParams>,
) -> Result<Response> {
    let pool = pool(&state)?;
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(params.days as i64)).to_rfc3339();

    // SQLite strftime truncates timestamp to hour bucket
    let rows = sqlx::query_as::<_, (String, i64, i64, f64, i64)>(
        "SELECT strftime('%Y-%m-%dT%H:00:00Z', timestamp) as hour, \
                COUNT(*) as request_count, \
                COALESCE(SUM(input_tokens + output_tokens), 0) as total_tokens, \
                COALESCE(SUM(estimated_cost_usd), 0.0) as total_cost, \
                SUM(CASE WHEN status = 'Failure' THEN 1 ELSE 0 END) as error_count \
         FROM inference_events WHERE timestamp >= ? \
         GROUP BY hour \
         ORDER BY hour ASC",
    )
    .bind(&cutoff)
    .fetch_all(pool)
    .await
    .map_err(|e| PrismError::Internal(e.to_string()))?;

    let result: Vec<TimeseriesBucket> = rows
        .into_iter()
        .map(|(hour, req, tokens, cost, errors)| TimeseriesBucket {
            hour,
            request_count: req as u64,
            total_tokens: tokens as u64,
            total_cost_usd: cost,
            error_count: errors as u64,
        })
        .collect();

    Ok(Json(result).into_response())
}
