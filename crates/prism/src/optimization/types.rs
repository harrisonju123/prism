use serde::{Deserialize, Serialize};

/// An optimization job targeting a specific prompt template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationJob {
    pub id: uuid::Uuid,
    pub prompt_name: String,
    pub status: OptimizationStatus,
    pub candidates: Vec<PromptCandidate>,
    pub best_candidate: Option<usize>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OptimizationStatus {
    Pending,
    Analyzing,
    Generating,
    Evaluating,
    Complete,
    Failed,
}

/// A candidate prompt generated during optimization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCandidate {
    pub index: usize,
    pub content: String,
    pub rationale: String,
    pub score: Option<f64>,
    pub eval_count: u32,
}

/// Request to start a prompt optimization job.
#[derive(Debug, Deserialize)]
pub struct OptimizeRequest {
    pub prompt_name: String,
    /// Number of candidates to generate (default: 3).
    #[serde(default = "default_num_candidates")]
    pub num_candidates: usize,
    /// Model to use for generating candidates.
    #[serde(default = "default_optimizer_model")]
    pub optimizer_model: String,
    /// Model to use for evaluating candidates.
    #[serde(default = "default_eval_model")]
    pub eval_model: String,
    /// Number of evaluation samples per candidate.
    #[serde(default = "default_eval_samples")]
    pub eval_samples: usize,
}

fn default_num_candidates() -> usize {
    3
}
fn default_optimizer_model() -> String {
    "gpt-4o".to_string()
}
fn default_eval_model() -> String {
    "gpt-4o-mini".to_string()
}
fn default_eval_samples() -> usize {
    5
}

/// Response from the optimization endpoint.
#[derive(Debug, Serialize)]
pub struct OptimizeResponse {
    pub job: OptimizationJob,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_optimize_request_defaults() {
        let json = r#"{"prompt_name": "my-prompt"}"#;
        let req: OptimizeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.prompt_name, "my-prompt");
        assert_eq!(req.num_candidates, 3);
        assert_eq!(req.eval_samples, 5);
    }

    #[test]
    fn optimization_status_variants() {
        assert_eq!(OptimizationStatus::Pending, OptimizationStatus::Pending);
        assert_ne!(OptimizationStatus::Pending, OptimizationStatus::Complete);
    }
}
