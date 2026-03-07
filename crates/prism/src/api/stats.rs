use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SummaryParams {
    #[serde(default = "default_period_days")]
    pub period_days: u32,
    #[serde(default = "default_group_by")]
    pub group_by: String,
}

#[derive(Debug, Deserialize)]
pub struct TimeseriesParams {
    #[serde(default = "default_period_days")]
    pub period_days: u32,
    #[serde(default = "default_interval")]
    pub interval: String,
    #[serde(default = "default_metric")]
    pub metric: String,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TopTracesParams {
    #[serde(default = "default_period_days")]
    pub period_days: u32,
    #[serde(default = "default_sort_by")]
    pub sort_by: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

#[derive(Debug, Deserialize)]
pub struct WasteScoreParams {
    #[serde(default = "default_period_days")]
    pub period_days: u32,
}

fn default_period_days() -> u32 {
    7
}
fn default_group_by() -> String {
    "model".into()
}
fn default_interval() -> String {
    "1h".into()
}
fn default_metric() -> String {
    "cost".into()
}
fn default_sort_by() -> String {
    "cost".into()
}
fn default_limit() -> u32 {
    10
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SummaryResponse {
    pub period_days: u32,
    pub total_requests: u64,
    pub total_cost_usd: f64,
    pub total_tokens: u64,
    pub failure_rate: f64,
    pub groups: Vec<StatGroup>,
}

#[derive(Debug, Serialize)]
pub struct StatGroup {
    pub key: String,
    pub request_count: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub avg_cost_per_request_usd: f64,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub failure_count: u64,
}

#[derive(Debug, Serialize)]
pub struct TimeseriesResponse {
    pub metric: String,
    pub interval: String,
    pub data: Vec<TimeseriesPoint>,
}

#[derive(Debug, Serialize)]
pub struct TimeseriesPoint {
    pub timestamp: String,
    pub value: f64,
}

#[derive(Debug, Serialize)]
pub struct TopTracesResponse {
    pub traces: Vec<TraceInfo>,
}

#[derive(Debug, Serialize)]
pub struct TraceInfo {
    pub trace_id: String,
    pub total_cost_usd: f64,
    pub total_tokens: u64,
    pub total_latency_ms: f64,
    pub event_count: u64,
    pub models_used: Vec<String>,
    pub agent_framework: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WasteScoreResponse {
    pub period_days: u32,
    pub waste_score: f64,
    pub total_cost_usd: f64,
    pub estimated_waste_usd: f64,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/stats/summary
pub async fn summary(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SummaryParams>,
) -> Result<Response> {
    let group_by = match params.group_by.as_str() {
        "model" | "provider" | "task_type" => params.group_by.clone(),
        _ => {
            return Err(PrismError::BadRequest(
                "group_by must be one of: model, provider, task_type".into(),
            ));
        }
    };

    let client = reqwest::Client::new();
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;

    let query = format!(
        "SELECT {group_by} as key, \
                count() as request_count, \
                sum(estimated_cost_usd) as total_cost_usd, \
                avg(latency_ms) as avg_latency_ms, \
                quantile(0.95)(latency_ms) as p95_latency_ms, \
                avg(estimated_cost_usd) as avg_cost_per_request_usd, \
                sum(input_tokens) as total_prompt_tokens, \
                sum(output_tokens) as total_completion_tokens, \
                countIf(status = 'failure') as failure_count \
         FROM {db}.inference_events \
         WHERE timestamp >= now() - INTERVAL {days} DAY \
         GROUP BY {group_by} \
         ORDER BY total_cost_usd DESC \
         FORMAT JSONEachRow",
        group_by = group_by,
        db = ch_db,
        days = params.period_days
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

    let mut groups = Vec::new();
    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            groups.push(StatGroup {
                key: v
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                request_count: v.get("request_count").and_then(|v| v.as_u64()).unwrap_or(0),
                total_cost_usd: v
                    .get("total_cost_usd")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                avg_latency_ms: v
                    .get("avg_latency_ms")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                p95_latency_ms: v
                    .get("p95_latency_ms")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                avg_cost_per_request_usd: v
                    .get("avg_cost_per_request_usd")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                total_prompt_tokens: v
                    .get("total_prompt_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                total_completion_tokens: v
                    .get("total_completion_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                failure_count: v.get("failure_count").and_then(|v| v.as_u64()).unwrap_or(0),
            });
        }
    }

    let total_requests: u64 = groups.iter().map(|g| g.request_count).sum();
    let total_cost: f64 = groups.iter().map(|g| g.total_cost_usd).sum();
    let total_tokens: u64 = groups
        .iter()
        .map(|g| g.total_prompt_tokens + g.total_completion_tokens)
        .sum();
    let total_failures: u64 = groups.iter().map(|g| g.failure_count).sum();
    let failure_rate = if total_requests > 0 {
        total_failures as f64 / total_requests as f64
    } else {
        0.0
    };

    Ok(Json(SummaryResponse {
        period_days: params.period_days,
        total_requests,
        total_cost_usd: total_cost,
        total_tokens,
        failure_rate,
        groups,
    })
    .into_response())
}

/// GET /api/v1/stats/timeseries
pub async fn timeseries(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TimeseriesParams>,
) -> Result<Response> {
    let interval_expr = match params.interval.as_str() {
        "1h" => "toStartOfHour(timestamp)",
        "6h" => "toStartOfInterval(timestamp, INTERVAL 6 HOUR)",
        "1d" => "toStartOfDay(timestamp)",
        _ => {
            return Err(PrismError::BadRequest(
                "interval must be one of: 1h, 6h, 1d".into(),
            ));
        }
    };

    let metric_expr = match params.metric.as_str() {
        "cost" => "sum(estimated_cost_usd)",
        "requests" => "count()",
        "latency" => "avg(latency_ms)",
        "tokens" => "sum(total_tokens)",
        _ => {
            return Err(PrismError::BadRequest(
                "metric must be one of: cost, requests, latency, tokens".into(),
            ));
        }
    };

    let model_filter = params
        .model
        .as_deref()
        .map(|m| {
            // Sanitize: only allow alphanumeric, dash, dot, underscore, colon, slash
            let sanitized: String = m
                .chars()
                .filter(|c| {
                    c.is_alphanumeric()
                        || *c == '-'
                        || *c == '.'
                        || *c == '_'
                        || *c == ':'
                        || *c == '/'
                })
                .collect();
            format!("AND model = '{}'", sanitized)
        })
        .unwrap_or_default();

    let client = reqwest::Client::new();
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;

    let query = format!(
        "SELECT toString({interval}) as timestamp, \
                {metric} as value \
         FROM {db}.inference_events \
         WHERE timestamp >= now() - INTERVAL {days} DAY \
           {model_filter} \
         GROUP BY timestamp \
         ORDER BY timestamp ASC \
         FORMAT JSONEachRow",
        interval = interval_expr,
        metric = metric_expr,
        db = ch_db,
        days = params.period_days,
        model_filter = model_filter,
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
            data.push(TimeseriesPoint {
                timestamp: v
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                value: v.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0),
            });
        }
    }

    Ok(Json(TimeseriesResponse {
        metric: params.metric,
        interval: params.interval,
        data,
    })
    .into_response())
}

/// GET /api/v1/stats/top-traces
pub async fn top_traces(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TopTracesParams>,
) -> Result<Response> {
    let sort_expr = match params.sort_by.as_str() {
        "cost" => "total_cost_usd",
        "tokens" => "total_tokens",
        "latency" => "total_latency_ms",
        _ => {
            return Err(PrismError::BadRequest(
                "sort_by must be one of: cost, tokens, latency".into(),
            ));
        }
    };

    let limit = params.limit.min(100).max(1);

    let client = reqwest::Client::new();
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;

    let query = format!(
        "SELECT trace_id, \
                sum(estimated_cost_usd) as total_cost_usd, \
                sum(total_tokens) as total_tokens, \
                sum(latency_ms) as total_latency_ms, \
                count() as event_count, \
                groupUniqArray(model) as models_used, \
                any(agent_framework) as agent_framework \
         FROM {db}.inference_events \
         WHERE timestamp >= now() - INTERVAL {days} DAY \
           AND trace_id IS NOT NULL \
           AND trace_id != '' \
         GROUP BY trace_id \
         ORDER BY {sort} DESC \
         LIMIT {limit} \
         FORMAT JSONEachRow",
        db = ch_db,
        days = params.period_days,
        sort = sort_expr,
        limit = limit,
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

    let mut traces = Vec::new();
    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            traces.push(TraceInfo {
                trace_id: v
                    .get("trace_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                total_cost_usd: v
                    .get("total_cost_usd")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                total_tokens: v.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                total_latency_ms: v
                    .get("total_latency_ms")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                event_count: v.get("event_count").and_then(|v| v.as_u64()).unwrap_or(0),
                models_used: v
                    .get("models_used")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                agent_framework: v
                    .get("agent_framework")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            });
        }
    }

    Ok(Json(TopTracesResponse { traces }).into_response())
}

// ---------------------------------------------------------------------------
// Task type stats response
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TaskTypeParams {
    #[serde(default = "default_period_days")]
    pub period_days: u32,
}

#[derive(Debug, Serialize)]
pub struct TaskTypeStatsResponse {
    pub period_days: u32,
    pub task_types: Vec<TaskTypeStat>,
}

#[derive(Debug, Serialize)]
pub struct TaskTypeStat {
    pub task_type: String,
    pub request_count: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
}

/// GET /api/v1/stats/task-types
pub async fn task_type_stats(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TaskTypeParams>,
) -> Result<Response> {
    let client = reqwest::Client::new();
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;

    let query = format!(
        "SELECT task_type, \
                sum(request_count) AS request_count, \
                sum(total_cost_usd) AS total_cost_usd, \
                avg(avg_latency_ms) AS avg_latency_ms, \
                avg(p95_latency_ms) AS p95_latency_ms \
         FROM {db}.hourly_task_stats \
         WHERE hour >= now() - INTERVAL {days} DAY \
         GROUP BY task_type \
         ORDER BY request_count DESC \
         FORMAT JSONEachRow",
        db = ch_db,
        days = params.period_days,
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

    let mut task_types = Vec::new();
    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            task_types.push(TaskTypeStat {
                task_type: v
                    .get("task_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                request_count: v.get("request_count").and_then(|v| v.as_u64()).unwrap_or(0),
                total_cost_usd: v
                    .get("total_cost_usd")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                avg_latency_ms: v
                    .get("avg_latency_ms")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                p95_latency_ms: v
                    .get("p95_latency_ms")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
            });
        }
    }

    Ok(Json(TaskTypeStatsResponse {
        period_days: params.period_days,
        task_types,
    })
    .into_response())
}

/// GET /api/v1/stats/waste-score
pub async fn waste_score(
    State(state): State<Arc<AppState>>,
    Query(params): Query<WasteScoreParams>,
) -> Result<Response> {
    if !state.config.waste.enabled {
        return Err(PrismError::BadRequest("waste detection is disabled".into()));
    }

    let report = crate::waste::detector::generate_waste_report(
        &state.config.clickhouse.url,
        &state.config.clickhouse.database,
        &state.fitness_cache,
        &state.config.waste,
        params.period_days,
    )
    .await
    .map_err(|e| PrismError::Internal(format!("waste score computation failed: {e}")))?;

    Ok(Json(WasteScoreResponse {
        period_days: params.period_days,
        waste_score: report.waste_percentage,
        total_cost_usd: report.total_cost_usd,
        estimated_waste_usd: report.estimated_waste_usd,
    })
    .into_response())
}
