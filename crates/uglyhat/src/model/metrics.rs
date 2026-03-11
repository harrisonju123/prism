use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetrics {
    pub agent: String,
    pub tasks_claimed: i64,
    pub tasks_done: i64,
    pub avg_completion_mins: f64,
    pub tasks_blocked: i64,
}
