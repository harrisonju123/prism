use crate::types::TaskType;
use serde::Serialize;

// Re-export shared types from prism-types
pub use prism_types::{FallbackEntry, RoutingRule, SelectionCriteria};

/// A set of routing rules.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
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
    /// Fallback chain from the matched routing rule.
    #[serde(default)]
    pub fallback_chain: Vec<FallbackEntry>,
}

/// Fitness data for a model on a specific task type.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
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
    TaskType::FillInTheMiddle,
];
