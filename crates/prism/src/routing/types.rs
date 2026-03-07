use crate::types::TaskType;
use serde::{Deserialize, Serialize};

/// How to select from candidate models.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionCriteria {
    CheapestAboveQuality,
    FastestAboveQuality,
    HighestQualityUnderCost,
    BestValue,
}

impl Default for SelectionCriteria {
    fn default() -> Self {
        Self::CheapestAboveQuality
    }
}

/// A single routing rule from config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    /// Task type to match, or "*" for catch-all
    pub task_type: String,
    #[serde(default)]
    pub criteria: SelectionCriteria,
    /// Minimum quality score (0.0-1.0)
    #[serde(default = "default_min_quality")]
    pub min_quality: f64,
    pub max_cost_per_1k: Option<f64>,
    pub max_latency_ms: Option<u32>,
    pub fallback: Option<String>,
}

fn default_min_quality() -> f64 {
    0.55
}

/// A set of routing rules.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingPolicy {
    pub rules: Vec<RoutingRule>,
    #[serde(default)]
    pub version: u32,
}

/// The outcome of a routing decision.
#[derive(Debug, Clone, Serialize)]
pub struct RoutingDecision {
    pub selected_model: String,
    pub reason: String,
    pub was_overridden: bool,
    pub policy_rule_id: Option<usize>,
    pub task_type: TaskType,
    pub confidence: f64,
}

/// Fitness data for a model on a specific task type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitnessEntry {
    pub task_type: TaskType,
    pub model: String,
    pub avg_quality: f64,
    pub avg_cost_per_1k: f64,
    pub avg_latency_ms: f64,
    pub sample_size: u32,
}

/// Quality floor -- never route to a model below this quality.
pub const QUALITY_FLOOR: f64 = 0.30;

/// Hard task types that deserve higher quality models.
pub const HARD_TASKS: &[TaskType] = &[
    TaskType::CodeGeneration,
    TaskType::CodeReview,
    TaskType::Reasoning,
    TaskType::Architecture,
    TaskType::Debugging,
    TaskType::Refactoring,
];
