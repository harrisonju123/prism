use prism_types::{
    AgentMetricsResponse, PolicyResponse, QualityTrendsResponse, RoutingSavingsResponse,
    SessionEfficiencyResponse, SummaryResponse, TaskTypeStatsResponse, WasteNudgesResponse,
    WasteScoreResponse,
};

#[derive(Default)]
pub struct DashboardData {
    pub summary: Option<SummaryResponse>,
    pub waste_score: Option<WasteScoreResponse>,
    pub waste_nudges: Option<WasteNudgesResponse>,
    pub task_types: Option<TaskTypeStatsResponse>,
    pub policy: Option<PolicyResponse>,
    pub agent_metrics: Option<AgentMetricsResponse>,
    pub quality_trends: Option<QualityTrendsResponse>,
    pub routing_savings: Option<RoutingSavingsResponse>,
    pub session_efficiency: Option<SessionEfficiencyResponse>,
}
