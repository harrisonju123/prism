use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// A parsed MCP tool call extracted from a completion.
#[derive(Debug, Clone, Serialize)]
pub struct McpCall {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub trace_id: String,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub server: String,
    pub method: String,
    pub tool_name: String,
    pub args_hash: String,
    pub inference_id: Uuid,
    pub model: String,
    pub estimated_cost: f64,
}

/// A node in the execution DAG for a trace.
#[derive(Debug, Clone, Serialize)]
pub struct DagNode {
    pub id: Uuid,
    pub tool_name: String,
    pub server: String,
    pub method: String,
    pub parent_id: Option<Uuid>,
    pub depth: u32,
    pub estimated_cost: f64,
}

/// Execution DAG for a single trace.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionDag {
    pub trace_id: String,
    pub nodes: Vec<DagNode>,
    pub total_cost: f64,
    pub max_depth: u32,
}
