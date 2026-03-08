use prism_types::{
    AgentMetricsResponse, PolicyResponse, SummaryResponse, TaskTypeStatsResponse,
    WasteScoreResponse,
};

#[derive(Default)]
pub struct DashboardData {
    pub summary: Option<SummaryResponse>,
    pub waste_score: Option<WasteScoreResponse>,
    pub task_types: Option<TaskTypeStatsResponse>,
    pub policy: Option<PolicyResponse>,
    pub agent_metrics: Option<AgentMetricsResponse>,
}
