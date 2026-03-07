use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};

use crate::config::Config;
use crate::providers::ProviderRegistry;
use crate::proxy::cost::compute_cost;
use crate::proxy::handler::resolve_model;
use crate::types::{ChatCompletionRequest, Message};
use crate::workflows::types::{ExecuteWorkflowRequest, NodeDefinition, NodeResult, WorkflowResult};

/// Build a petgraph DAG from the workflow definition, returning (graph, node_id → index map).
fn build_dag(
    nodes: &[NodeDefinition],
) -> Result<
    (
        DiGraph<usize, ()>,
        HashMap<String, NodeIndex>,
        Vec<NodeIndex>,
    ),
    String,
> {
    let mut graph = DiGraph::new();
    let mut index_map: HashMap<String, NodeIndex> = HashMap::new();

    // Add nodes
    for (i, node) in nodes.iter().enumerate() {
        let idx = graph.add_node(i);
        index_map.insert(node.id.clone(), idx);
    }

    // Add edges (parent → child)
    for node in nodes {
        let child_idx = index_map[&node.id];
        for dep in &node.depends_on {
            let parent_idx = index_map
                .get(dep)
                .ok_or_else(|| format!("node '{}' depends on unknown node '{}'", node.id, dep))?;
            graph.add_edge(*parent_idx, child_idx, ());
        }
    }

    // Topological sort (also validates no cycles)
    let topo = toposort(&graph, None).map_err(|_| "workflow contains a cycle".to_string())?;

    Ok((graph, index_map, topo))
}

/// Expand template variables: replace `{{var}}` with values from outputs + variables.
fn expand_template(
    template: &str,
    outputs: &HashMap<String, String>,
    variables: &HashMap<String, String>,
) -> String {
    let mut result = template.to_string();
    for (key, value) in outputs.iter().chain(variables.iter()) {
        let placeholder = format!("{{{{{}}}}}", key);
        result = result.replace(&placeholder, value);
    }
    result
}

/// Evaluate a simple condition string. Supports `{{node_id}}.contains('keyword')`.
fn evaluate_condition(condition: &str, outputs: &HashMap<String, String>) -> bool {
    // Parse pattern: {{node_id}}.contains('keyword')
    let Some(open) = condition.find("{{") else {
        return true;
    };
    let Some(dot_pos) = condition.find("}}.contains('") else {
        return true;
    };
    if open + 2 > dot_pos {
        return true;
    }
    let node_id = &condition[open + 2..dot_pos];
    let keyword_start = dot_pos + "}}.contains('".len();
    let Some(keyword_len) = condition[keyword_start..].find("')") else {
        return true;
    };
    let keyword = &condition[keyword_start..keyword_start + keyword_len];

    outputs
        .get(node_id)
        .is_some_and(|output| output.contains(keyword))
}

/// Execute a workflow against the provider registry.
pub async fn execute_workflow(
    request: &ExecuteWorkflowRequest,
    config: &Config,
    providers: &Arc<ProviderRegistry>,
) -> Result<WorkflowResult, String> {
    let config = Arc::new(config.clone());
    let wf = &request.workflow;

    if wf.nodes.is_empty() {
        return Err("workflow has no nodes".to_string());
    }

    let (graph, _index_map, topo) = build_dag(&wf.nodes)?;

    let mut outputs: HashMap<String, String> = HashMap::new();
    let mut node_results: Vec<NodeResult> = Vec::new();
    let workflow_start = Instant::now();

    // Group nodes by depth for concurrent execution
    let mut depth_map: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut node_depths: HashMap<NodeIndex, usize> = HashMap::new();

    for &idx in &topo {
        let depth = graph
            .neighbors_directed(idx, petgraph::Direction::Incoming)
            .map(|parent| node_depths.get(&parent).copied().unwrap_or(0) + 1)
            .max()
            .unwrap_or(0);
        node_depths.insert(idx, depth);
        depth_map
            .entry(depth)
            .or_default()
            .push(*graph.node_weight(idx).unwrap());
    }

    let max_depth = depth_map.keys().max().copied().unwrap_or(0);

    for depth in 0..=max_depth {
        let Some(node_indices) = depth_map.get(&depth) else {
            continue;
        };

        // Execute nodes at the same depth concurrently
        let mut join_set = tokio::task::JoinSet::new();

        for &node_idx in node_indices {
            let node = wf.nodes[node_idx].clone();
            let variables = request.variables.clone();
            let outputs_snap = outputs.clone();
            let config = config.clone();
            let providers = providers.clone();

            join_set.spawn(async move {
                execute_node(&node, &outputs_snap, &variables, &config, &providers).await
            });
        }

        while let Some(result) = join_set.join_next().await {
            let node_result = result
                .map_err(|e| format!("join error: {e}"))?
                .map_err(|e| format!("node execution error: {e}"))?;

            if !node_result.skipped {
                outputs.insert(node_result.node_id.clone(), node_result.output.clone());
            }
            node_results.push(node_result);
        }
    }

    let total_latency_ms = workflow_start.elapsed().as_millis() as u32;
    let total_cost: f64 = node_results.iter().map(|r| r.estimated_cost).sum();

    // Final output = output of the topological sink node (node with no dependents)
    let sink_node_id = topo
        .last()
        .and_then(|&idx| graph.node_weight(idx))
        .map(|&i| wf.nodes[i].id.clone());
    let final_output = sink_node_id
        .and_then(|id| outputs.get(&id).cloned())
        .unwrap_or_default();

    Ok(WorkflowResult {
        workflow_name: wf.name.clone(),
        node_results,
        total_latency_ms,
        total_cost,
        final_output,
    })
}

async fn execute_node(
    node: &NodeDefinition,
    outputs: &HashMap<String, String>,
    variables: &HashMap<String, String>,
    config: &Arc<Config>,
    providers: &Arc<ProviderRegistry>,
) -> Result<NodeResult, String> {
    // Check condition
    if let Some(ref condition) = node.condition {
        if !evaluate_condition(condition, outputs) {
            return Ok(NodeResult {
                node_id: node.id.clone(),
                model: node.model.clone(),
                output: String::new(),
                latency_ms: 0,
                input_tokens: 0,
                output_tokens: 0,
                estimated_cost: 0.0,
                skipped: true,
            });
        }
    }

    let prompt = expand_template(&node.prompt_template, outputs, variables);
    let (provider_name, model_id) =
        resolve_model(config, &node.model).map_err(|e| format!("{e}"))?;
    let provider = providers.get(&provider_name).map_err(|e| format!("{e}"))?;

    let request = ChatCompletionRequest {
        model: node.model.clone(),
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(serde_json::Value::String(prompt)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: serde_json::Map::new(),
        }],
        temperature: node.temperature,
        max_tokens: node.max_tokens,
        stream: false,
        ..Default::default()
    };

    let start = Instant::now();
    let response = provider
        .chat_completion(&request, &model_id)
        .await
        .map_err(|e| format!("provider error for node '{}': {e}", node.id))?;

    let latency_ms = start.elapsed().as_millis() as u32;

    match response {
        crate::types::ProviderResponse::Complete(resp) => {
            let output = resp
                .choices
                .first()
                .and_then(|c| c.message.content.as_ref())
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let usage = resp.usage.clone().unwrap_or_default();
            let cost = compute_cost(&resp.model, &usage);

            Ok(NodeResult {
                node_id: node.id.clone(),
                model: resp.model,
                output,
                latency_ms,
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
                estimated_cost: cost,
                skipped: false,
            })
        }
        crate::types::ProviderResponse::Stream(_) => Err(format!(
            "node '{}': streaming not supported in workflows",
            node.id
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_dag_linear() {
        let nodes = vec![
            NodeDefinition {
                id: "a".into(),
                label: String::new(),
                model: "m".into(),
                prompt_template: "t".into(),
                depends_on: vec![],
                condition: None,
                max_tokens: None,
                temperature: None,
            },
            NodeDefinition {
                id: "b".into(),
                label: String::new(),
                model: "m".into(),
                prompt_template: "t".into(),
                depends_on: vec!["a".into()],
                condition: None,
                max_tokens: None,
                temperature: None,
            },
        ];
        let (graph, _, _) = build_dag(&nodes).unwrap();
        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn build_dag_cycle_detected() {
        let nodes = vec![
            NodeDefinition {
                id: "a".into(),
                label: String::new(),
                model: "m".into(),
                prompt_template: "t".into(),
                depends_on: vec!["b".into()],
                condition: None,
                max_tokens: None,
                temperature: None,
            },
            NodeDefinition {
                id: "b".into(),
                label: String::new(),
                model: "m".into(),
                prompt_template: "t".into(),
                depends_on: vec!["a".into()],
                condition: None,
                max_tokens: None,
                temperature: None,
            },
        ];
        assert!(build_dag(&nodes).is_err());
    }

    #[test]
    fn build_dag_unknown_dependency() {
        let nodes = vec![NodeDefinition {
            id: "a".into(),
            label: String::new(),
            model: "m".into(),
            prompt_template: "t".into(),
            depends_on: vec!["nonexistent".into()],
            condition: None,
            max_tokens: None,
            temperature: None,
        }];
        assert!(build_dag(&nodes).is_err());
    }

    #[test]
    fn expand_template_replaces_vars() {
        let mut outputs = HashMap::new();
        outputs.insert("step1".to_string(), "hello world".to_string());
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "test".to_string());

        let result = expand_template("{{step1}} from {{name}}", &outputs, &vars);
        assert_eq!(result, "hello world from test");
    }

    #[test]
    fn evaluate_condition_contains() {
        let mut outputs = HashMap::new();
        outputs.insert("summary".to_string(), "The code has bugs".to_string());

        assert!(evaluate_condition("{{summary}}.contains('bugs')", &outputs));
        assert!(!evaluate_condition(
            "{{summary}}.contains('perfect')",
            &outputs
        ));
    }

    #[test]
    fn evaluate_condition_missing_node() {
        let outputs = HashMap::new();
        assert!(!evaluate_condition("{{missing}}.contains('x')", &outputs));
    }

    #[test]
    fn build_dag_diamond() {
        //   a
        //  / \
        // b   c
        //  \ /
        //   d
        let nodes = vec![
            NodeDefinition {
                id: "a".into(),
                label: String::new(),
                model: "m".into(),
                prompt_template: "t".into(),
                depends_on: vec![],
                condition: None,
                max_tokens: None,
                temperature: None,
            },
            NodeDefinition {
                id: "b".into(),
                label: String::new(),
                model: "m".into(),
                prompt_template: "t".into(),
                depends_on: vec!["a".into()],
                condition: None,
                max_tokens: None,
                temperature: None,
            },
            NodeDefinition {
                id: "c".into(),
                label: String::new(),
                model: "m".into(),
                prompt_template: "t".into(),
                depends_on: vec!["a".into()],
                condition: None,
                max_tokens: None,
                temperature: None,
            },
            NodeDefinition {
                id: "d".into(),
                label: String::new(),
                model: "m".into(),
                prompt_template: "t".into(),
                depends_on: vec!["b".into(), "c".into()],
                condition: None,
                max_tokens: None,
                temperature: None,
            },
        ];
        let (graph, _, _) = build_dag(&nodes).unwrap();
        assert_eq!(graph.node_count(), 4);
        assert_eq!(graph.edge_count(), 4);
    }
}
