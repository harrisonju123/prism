pub mod judge;
pub mod refresh;
pub mod sampler;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{ChatCompletionRequest, TaskType};

/// Sent from handler to sampler via mpsc channel.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BenchmarkRequest {
    pub inference_id: Uuid,
    pub request: ChatCompletionRequest,
    pub original_model: String,
    pub original_completion: String,
    pub original_cost: f64,
    pub original_latency_ms: u32,
    pub task_type: Option<TaskType>,
    pub prompt_hash: String,
}

/// Stored to ClickHouse for benchmark results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkEvent {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub inference_id: Uuid,
    pub task_type: Option<TaskType>,
    pub original_model: String,
    pub benchmark_model: String,
    pub judge_model: String,
    pub original_score: f64,
    pub benchmark_score: f64,
    pub benchmark_cost: f64,
    pub benchmark_latency_ms: u32,
    pub judge_cost: f64,
    pub prompt_hash: String,
    pub status: String,
}
