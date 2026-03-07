use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A workflow is a named DAG of LLM steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub nodes: Vec<NodeDefinition>,
}

/// A single step in the workflow DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDefinition {
    /// Unique identifier for this node within the workflow.
    pub id: String,
    /// Human-readable label.
    #[serde(default)]
    pub label: String,
    /// Which model to use for this step.
    pub model: String,
    /// The prompt template. Use `{{parent_id}}` to reference parent outputs.
    pub prompt_template: String,
    /// IDs of parent nodes whose outputs this node depends on.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Optional condition: only execute if this evaluates to true.
    /// Format: `"{{node_id}}.contains('keyword')"` — simple string matching.
    #[serde(default)]
    pub condition: Option<String>,
    /// Max tokens for this step.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Temperature for this step.
    #[serde(default)]
    pub temperature: Option<f64>,
}

/// Result of executing a single node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResult {
    pub node_id: String,
    pub model: String,
    pub output: String,
    pub latency_ms: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub estimated_cost: f64,
    pub skipped: bool,
}

/// Result of executing the entire workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub workflow_name: String,
    pub node_results: Vec<NodeResult>,
    pub total_latency_ms: u32,
    pub total_cost: f64,
    pub final_output: String,
}

/// Request to execute a workflow.
#[derive(Debug, Deserialize)]
pub struct ExecuteWorkflowRequest {
    pub workflow: WorkflowDefinition,
    /// Variables to substitute into prompt templates: `{{var_name}}`.
    #[serde(default)]
    pub variables: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_workflow() {
        let json = r#"{
            "name": "summarize-then-critique",
            "nodes": [
                {
                    "id": "summarize",
                    "model": "gpt-4o-mini",
                    "prompt_template": "Summarize: {{input}}"
                },
                {
                    "id": "critique",
                    "model": "gpt-4o",
                    "prompt_template": "Critique this summary: {{summarize}}",
                    "depends_on": ["summarize"]
                }
            ]
        }"#;
        let wf: WorkflowDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(wf.nodes.len(), 2);
        assert_eq!(wf.nodes[1].depends_on, vec!["summarize"]);
    }

    #[test]
    fn deserialize_execute_request() {
        let json = r#"{
            "workflow": {
                "name": "test",
                "nodes": [
                    {"id": "a", "model": "gpt-4o", "prompt_template": "Hello {{name}}"}
                ]
            },
            "variables": {"name": "world"}
        }"#;
        let req: ExecuteWorkflowRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.variables.get("name").unwrap(), "world");
    }
}
