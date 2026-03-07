use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::mcp::types::{DagNode, ExecutionDag};
use crate::proxy::handler::AppState;

#[derive(Debug, Deserialize)]
pub struct TraceQuery {
    pub trace_id: String,
}

#[derive(Debug, Serialize)]
pub struct McpTraceResponse {
    pub dag: ExecutionDag,
}

#[derive(Debug, Deserialize)]
struct McpRow {
    id: String,
    server: String,
    method: String,
    tool_name: String,
    parent_span_id: Option<String>,
    estimated_cost: f64,
}

/// GET /api/v1/mcp/trace?trace_id=... — build execution DAG for a trace.
pub async fn mcp_trace(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TraceQuery>,
) -> Result<Response> {
    let client = reqwest::Client::new();
    let query = format!(
        "SELECT id, server, method, tool_name, parent_span_id, estimated_cost \
         FROM {db}.mcp_calls \
         WHERE trace_id = '{trace_id}' \
         ORDER BY timestamp ASC \
         FORMAT JSONEachRow",
        db = state.config.clickhouse.database,
        trace_id = params.trace_id,
    );

    let resp = client
        .post(&state.config.clickhouse.url)
        .body(query)
        .send()
        .await
        .map_err(|e| crate::error::PrismError::Internal(format!("clickhouse query failed: {e}")))?
        .text()
        .await
        .map_err(|e| {
            crate::error::PrismError::Internal(format!("clickhouse response read failed: {e}"))
        })?;

    let mut nodes: Vec<DagNode> = Vec::new();
    let mut total_cost = 0.0;

    for line in resp.lines() {
        let Ok(row) = serde_json::from_str::<McpRow>(line) else {
            continue;
        };

        let id = Uuid::parse_str(&row.id).unwrap_or_else(|_| Uuid::new_v4());
        let parent_id = row
            .parent_span_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok());

        // Compute depth: if parent exists in our nodes, depth = parent.depth + 1
        let depth = parent_id
            .and_then(|pid| nodes.iter().find(|n| n.id == pid))
            .map(|p| p.depth + 1)
            .unwrap_or(0);

        total_cost += row.estimated_cost;

        nodes.push(DagNode {
            id,
            tool_name: row.tool_name,
            server: row.server,
            method: row.method,
            parent_id,
            depth,
            estimated_cost: row.estimated_cost,
        });
    }

    let max_depth = nodes.iter().map(|n| n.depth).max().unwrap_or(0);

    let dag = ExecutionDag {
        trace_id: params.trace_id,
        nodes,
        total_cost,
        max_depth,
    };

    Ok(Json(McpTraceResponse { dag }).into_response())
}
