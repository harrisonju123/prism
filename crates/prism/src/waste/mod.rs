pub mod detector;
pub mod handler;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WasteCategory {
    ModelOverkill,
    Overspend,
    RedundantCalls,
    CacheMisses,
    ContextBloat,
    AgentLoops,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WasteSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize)]
pub struct WasteItem {
    pub category: WasteCategory,
    pub severity: WasteSeverity,
    pub affected_trace_ids: Vec<String>,
    pub call_count: u64,
    pub current_cost: f64,
    pub projected_cost: f64,
    pub savings: f64,
    pub description: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WasteReport {
    pub generated_at: String,
    pub period_days: u32,
    pub total_requests: u64,
    pub total_cost_usd: f64,
    pub estimated_waste_usd: f64,
    pub waste_percentage: f64,
    pub overkill: Vec<OverkillEntry>,
    pub overspend: Vec<OverspendEntry>,
    pub items: Vec<WasteItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverkillEntry {
    pub task_type: String,
    pub expensive_model: String,
    pub expensive_model_tier: u8,
    pub expensive_model_score: f64,
    pub cheaper_alternative: String,
    pub cheaper_model_tier: u8,
    pub cheaper_model_score: f64,
    pub request_count: u64,
    pub wasted_cost_usd: f64,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverspendEntry {
    pub task_type: String,
    pub model: String,
    pub request_count: u64,
    pub median_cost: f64,
    pub flagged_cost: f64,
    pub overspend_factor: f64,
    pub total_overspend_usd: f64,
}
