use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetrics {
    pub agent_name: String,
    pub total_sessions: i64,
    pub total_tasks_completed: i64,
    pub total_handoffs: i64,
}
