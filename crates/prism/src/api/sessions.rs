use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SessionListParams {
    pub thread_id: Option<String>,
    pub model: Option<String>,
    pub since: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    50
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SessionSummaryResponse {
    pub episode_id: String,
    pub model: String,
    pub task_type: Option<String>,
    pub thread_id: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub turn_count: u64,
    pub total_cost: f64,
    pub total_tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct SessionDetailResponse {
    pub episode_id: String,
    pub turns: Vec<TurnDetail>,
    pub feedback: Vec<FeedbackDetail>,
    pub total_cost: f64,
}

#[derive(Debug, Serialize)]
pub struct TurnDetail {
    pub inference_id: String,
    pub timestamp: String,
    pub model: String,
    pub task_type: Option<String>,
    pub routing_decision: Option<serde_json::Value>,
    pub cost: f64,
    pub latency_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct FeedbackDetail {
    pub id: String,
    pub timestamp: String,
    pub metric_name: String,
    pub metric_value: f64,
    pub metadata: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/sessions — list session summaries grouped by episode_id
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SessionListParams>,
) -> Result<Response> {
    let client = &state.http_client;
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;
    let limit = params.limit.min(200).max(1);

    let mut filters = Vec::new();
    filters.push("episode_id IS NOT NULL".to_string());

    if let Some(ref thread_id) = params.thread_id {
        let sanitized: String = thread_id
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-')
            .collect();
        filters.push(format!("thread_id = '{sanitized}'"));
    }
    if let Some(ref model) = params.model {
        let sanitized: String = model
            .chars()
            .filter(|c| {
                c.is_alphanumeric() || *c == '-' || *c == '.' || *c == '_' || *c == ':' || *c == '/'
            })
            .collect();
        filters.push(format!("model = '{sanitized}'"));
    }
    if let Some(ref since) = params.since {
        let sanitized: String = since
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == 'T' || *c == ':' || *c == 'Z')
            .collect();
        filters.push(format!("timestamp >= '{sanitized}'"));
    }

    let where_clause = filters.join(" AND ");

    let query = format!(
        "SELECT toString(episode_id) AS episode_id, \
                anyLast(model) AS model, \
                anyLast(task_type) AS task_type, \
                anyLast(session_id) AS session_id, \
                anyLast(thread_id) AS thread_id, \
                toString(min(timestamp)) AS started_at, \
                toString(max(timestamp)) AS ended_at, \
                count() AS turn_count, \
                sum(estimated_cost_usd) AS total_cost, \
                sum(total_tokens) AS total_tokens \
         FROM {db}.inference_events \
         WHERE {where_clause} \
         GROUP BY episode_id \
         ORDER BY started_at DESC \
         LIMIT {limit} \
         FORMAT JSONEachRow",
        db = ch_db,
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

    let mut sessions = Vec::new();
    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            sessions.push(SessionSummaryResponse {
                episode_id: v
                    .get("episode_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                model: v
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                task_type: v
                    .get("task_type")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                thread_id: v
                    .get("thread_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from),
                started_at: v
                    .get("started_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                ended_at: v
                    .get("ended_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                turn_count: v.get("turn_count").and_then(|v| v.as_u64()).unwrap_or(0),
                total_cost: v.get("total_cost").and_then(|v| v.as_f64()).unwrap_or(0.0),
                total_tokens: v.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            });
        }
    }

    Ok(Json(sessions).into_response())
}

/// GET /api/v1/sessions/:episode_id — per-turn detail for a session
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(episode_id): Path<String>,
) -> Result<Response> {
    let client = &state.http_client;
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;

    // Validate episode_id format
    let sanitized: String = episode_id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect();
    if sanitized.is_empty() {
        return Err(PrismError::BadRequest("invalid episode_id".into()));
    }

    // Fetch turns
    let turns_query = format!(
        "SELECT toString(id) AS inference_id, \
                toString(timestamp) AS timestamp, \
                model, task_type, routing_decision, \
                estimated_cost_usd AS cost, \
                latency_ms, input_tokens, output_tokens \
         FROM {db}.inference_events \
         WHERE episode_id = '{eid}' \
         ORDER BY timestamp ASC \
         FORMAT JSONEachRow",
        db = ch_db,
        eid = sanitized,
    );

    // Fetch feedback
    let feedback_query = format!(
        "SELECT toString(id) AS id, \
                toString(timestamp) AS timestamp, \
                metric_name, metric_value, metadata \
         FROM {db}.feedback_events \
         WHERE episode_id = '{eid}' \
         ORDER BY timestamp ASC \
         FORMAT JSONEachRow",
        db = ch_db,
        eid = sanitized,
    );

    let (turns_resp, feedback_resp) = tokio::join!(
        async {
            client
                .post(ch_url)
                .body(turns_query)
                .send()
                .await?
                .text()
                .await
        },
        async {
            client
                .post(ch_url)
                .body(feedback_query)
                .send()
                .await?
                .text()
                .await
        }
    );

    let turns_text =
        turns_resp.map_err(|e| PrismError::Internal(format!("clickhouse query failed: {e}")))?;
    let feedback_text =
        feedback_resp.map_err(|e| PrismError::Internal(format!("clickhouse query failed: {e}")))?;

    let mut turns = Vec::new();
    let mut total_cost = 0.0;
    for line in turns_text.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let cost = v.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
            total_cost += cost;
            turns.push(TurnDetail {
                inference_id: v
                    .get("inference_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                timestamp: v
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                model: v
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                task_type: v
                    .get("task_type")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                routing_decision: v.get("routing_decision").and_then(|v| {
                    if v.is_null() {
                        None
                    } else {
                        serde_json::from_str(v.as_str().unwrap_or("null")).ok()
                    }
                }),
                cost,
                latency_ms: v.get("latency_ms").and_then(|v| v.as_u64()).unwrap_or(0),
                input_tokens: v.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                output_tokens: v.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            });
        }
    }

    let mut feedback = Vec::new();
    for line in feedback_text.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            feedback.push(FeedbackDetail {
                id: v
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                timestamp: v
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                metric_name: v
                    .get("metric_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                metric_value: v
                    .get("metric_value")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                metadata: v
                    .get("metadata")
                    .and_then(|v| {
                        if let Some(s) = v.as_str() {
                            serde_json::from_str(s).ok()
                        } else {
                            Some(v.clone())
                        }
                    })
                    .unwrap_or(serde_json::Value::Object(Default::default())),
            });
        }
    }

    Ok(Json(SessionDetailResponse {
        episode_id: sanitized,
        turns,
        feedback,
        total_cost,
    })
    .into_response())
}
