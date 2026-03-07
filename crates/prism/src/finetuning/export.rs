use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};

use crate::error::{PrismError, Result};
use crate::finetuning::types::{ExportFormat, ExportRequest, ExportResponse};
use crate::proxy::handler::AppState;

/// POST /api/v1/finetuning/export — export training data as JSONL.
pub async fn export_training_data(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ExportRequest>,
) -> Result<Response> {
    let filters = &request.filters;
    let ch_url = &state.config.clickhouse.url;
    let ch_db = &state.config.clickhouse.database;

    // Build ClickHouse query with filters
    let mut conditions = vec![format!(
        "timestamp >= now() - INTERVAL {} DAY",
        filters.days
    )];
    conditions.push("status = 'Success'".to_string());

    if !filters.models.is_empty() {
        let models_list = filters
            .models
            .iter()
            .map(|m| {
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
                format!("'{}'", sanitized)
            })
            .collect::<Vec<_>>()
            .join(", ");
        conditions.push(format!("model IN ({})", models_list));
    }

    if !filters.task_types.is_empty() {
        let types_list = filters
            .task_types
            .iter()
            .map(|t| {
                let sanitized: String = t
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                format!("'{}'", sanitized)
            })
            .collect::<Vec<_>>()
            .join(", ");
        conditions.push(format!("task_type IN ({})", types_list));
    }

    if let Some(max_lat) = filters.max_latency_ms {
        conditions.push(format!("latency_ms <= {}", max_lat));
    }

    let where_clause = conditions.join(" AND ");

    let query = format!(
        "SELECT id, model, prompt_hash, completion_hash, \
         input_tokens, output_tokens, estimated_cost_usd, task_type \
         FROM {db}.inference_events \
         WHERE {where_clause} \
         ORDER BY timestamp DESC \
         LIMIT {limit} \
         FORMAT JSONEachRow",
        db = ch_db,
        where_clause = where_clause,
        limit = filters.max_samples,
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(ch_url)
        .body(query)
        .send()
        .await
        .map_err(|e| PrismError::Internal(format!("clickhouse query failed: {e}")))?
        .text()
        .await
        .map_err(|e| PrismError::Internal(format!("clickhouse read failed: {e}")))?;

    let rows: Vec<serde_json::Value> = resp
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    // Format output based on requested format
    let data: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| format_row(row, filters.format))
        .collect();

    Ok(Json(ExportResponse {
        format: filters.format.as_str().to_string(),
        total_samples: data.len(),
        data,
    })
    .into_response())
}

fn format_row(row: &serde_json::Value, format: ExportFormat) -> serde_json::Value {
    match format {
        ExportFormat::Openai => {
            // OpenAI fine-tuning format: {"messages": [...]}
            serde_json::json!({
                "messages": [
                    {
                        "role": "user",
                        "content": row.get("prompt_hash").and_then(|v| v.as_str()).unwrap_or("")
                    },
                    {
                        "role": "assistant",
                        "content": row.get("completion_hash").and_then(|v| v.as_str()).unwrap_or("")
                    }
                ],
                "_metadata": {
                    "model": row.get("model"),
                    "task_type": row.get("task_type"),
                    "cost": row.get("estimated_cost_usd")
                }
            })
        }
        ExportFormat::Anthropic => {
            serde_json::json!({
                "prompt": row.get("prompt_hash").and_then(|v| v.as_str()).unwrap_or(""),
                "completion": row.get("completion_hash").and_then(|v| v.as_str()).unwrap_or(""),
                "_metadata": {
                    "model": row.get("model"),
                    "task_type": row.get("task_type")
                }
            })
        }
        ExportFormat::Raw => row.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_row_openai() {
        let row = serde_json::json!({
            "prompt_hash": "abc",
            "completion_hash": "def",
            "model": "gpt-4o",
            "task_type": "Coding"
        });
        let result = format_row(&row, ExportFormat::Openai);
        let messages = result.get("messages").unwrap().as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn format_row_anthropic() {
        let row = serde_json::json!({
            "prompt_hash": "abc",
            "completion_hash": "def"
        });
        let result = format_row(&row, ExportFormat::Anthropic);
        assert_eq!(result["prompt"], "abc");
        assert_eq!(result["completion"], "def");
    }

    #[test]
    fn format_row_raw() {
        let row = serde_json::json!({"id": "123", "model": "gpt-4o"});
        let result = format_row(&row, ExportFormat::Raw);
        assert_eq!(result, row);
    }
}
