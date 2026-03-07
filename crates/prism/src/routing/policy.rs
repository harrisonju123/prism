use std::path::Path;

use super::types::{RoutingPolicy, RoutingRule, SelectionCriteria};

/// Load routing policy from a config-level rules vector.
/// If no rules provided, build a default policy.
pub fn load_policy(rules: Vec<RoutingRule>) -> RoutingPolicy {
    if rules.is_empty() {
        return build_default_policy();
    }
    RoutingPolicy { rules, version: 1 }
}

/// Load a routing policy from a YAML file.
pub fn load_policy_from_yaml(path: &Path) -> Result<RoutingPolicy, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read policy file: {e}"))?;
    parse_policy_yaml(&content)
}

/// Parse a YAML string into a RoutingPolicy.
pub fn parse_policy_yaml(yaml: &str) -> Result<RoutingPolicy, String> {
    let policy: RoutingPolicy =
        serde_yaml::from_str(yaml).map_err(|e| format!("invalid YAML policy: {e}"))?;
    validate_policy(&policy)?;
    Ok(policy)
}

/// Validate a routing policy for correctness.
pub fn validate_policy(policy: &RoutingPolicy) -> Result<(), String> {
    if policy.rules.is_empty() {
        return Err("policy must have at least one rule".into());
    }

    for (i, rule) in policy.rules.iter().enumerate() {
        if rule.task_type.is_empty() {
            return Err(format!("rule[{i}]: task_type cannot be empty"));
        }
        if rule.min_quality < 0.0 || rule.min_quality > 1.0 {
            return Err(format!(
                "rule[{i}]: min_quality must be between 0.0 and 1.0, got {}",
                rule.min_quality
            ));
        }
        if let Some(max_cost) = rule.max_cost_per_1k {
            if max_cost <= 0.0 {
                return Err(format!(
                    "rule[{i}]: max_cost_per_1k must be positive, got {max_cost}"
                ));
            }
        }
        if let Some(max_lat) = rule.max_latency_ms {
            if max_lat == 0 {
                return Err(format!("rule[{i}]: max_latency_ms must be > 0"));
            }
        }
    }

    // Check that catch-all "*" is last if present
    let catchall_positions: Vec<usize> = policy
        .rules
        .iter()
        .enumerate()
        .filter(|(_, r)| r.task_type == "*")
        .map(|(i, _)| i)
        .collect();

    if catchall_positions.len() > 1 {
        return Err("policy has multiple catch-all (*) rules".into());
    }
    if let Some(&pos) = catchall_positions.first() {
        if pos != policy.rules.len() - 1 {
            return Err("catch-all (*) rule must be the last rule".into());
        }
    }

    Ok(())
}

/// Build a default policy based on task difficulty.
fn build_default_policy() -> RoutingPolicy {
    let hard_types = [
        "code_generation",
        "code_review",
        "reasoning",
        "architecture",
        "debugging",
        "refactoring",
    ];

    let mut rules = Vec::new();

    // Hard tasks: higher quality floor
    for task_str in &hard_types {
        rules.push(RoutingRule {
            task_type: task_str.to_string(),
            criteria: SelectionCriteria::CheapestAboveQuality,
            min_quality: 0.70,
            max_cost_per_1k: None,
            max_latency_ms: None,
            fallback: None,
        });
    }

    // Catch-all
    rules.push(RoutingRule {
        task_type: "*".into(),
        criteria: SelectionCriteria::CheapestAboveQuality,
        min_quality: 0.55,
        max_cost_per_1k: None,
        max_latency_ms: None,
        fallback: None,
    });

    RoutingPolicy { rules, version: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_yaml_policy() {
        let yaml = r#"
version: 2
rules:
  - task_type: code_generation
    criteria: cheapest_above_quality
    min_quality: 0.75
    fallback: claude-sonnet-4
  - task_type: "*"
    criteria: best_value
    min_quality: 0.50
"#;
        let policy = parse_policy_yaml(yaml).unwrap();
        assert_eq!(policy.version, 2);
        assert_eq!(policy.rules.len(), 2);
        assert_eq!(policy.rules[0].task_type, "code_generation");
        assert_eq!(policy.rules[0].min_quality, 0.75);
        assert_eq!(policy.rules[1].criteria, SelectionCriteria::BestValue);
    }

    #[test]
    fn validate_empty_rules() {
        let policy = RoutingPolicy {
            rules: vec![],
            version: 0,
        };
        assert!(validate_policy(&policy).is_err());
    }

    #[test]
    fn validate_bad_quality() {
        let policy = RoutingPolicy {
            rules: vec![RoutingRule {
                task_type: "code_generation".into(),
                criteria: SelectionCriteria::CheapestAboveQuality,
                min_quality: 1.5,
                max_cost_per_1k: None,
                max_latency_ms: None,
                fallback: None,
            }],
            version: 0,
        };
        let err = validate_policy(&policy).unwrap_err();
        assert!(err.contains("min_quality"));
    }

    #[test]
    fn validate_catchall_not_last() {
        let policy = RoutingPolicy {
            rules: vec![
                RoutingRule {
                    task_type: "*".into(),
                    criteria: SelectionCriteria::CheapestAboveQuality,
                    min_quality: 0.5,
                    max_cost_per_1k: None,
                    max_latency_ms: None,
                    fallback: None,
                },
                RoutingRule {
                    task_type: "code_generation".into(),
                    criteria: SelectionCriteria::CheapestAboveQuality,
                    min_quality: 0.7,
                    max_cost_per_1k: None,
                    max_latency_ms: None,
                    fallback: None,
                },
            ],
            version: 0,
        };
        let err = validate_policy(&policy).unwrap_err();
        assert!(err.contains("catch-all"));
    }

    #[test]
    fn validate_valid_policy() {
        let policy = RoutingPolicy {
            rules: vec![
                RoutingRule {
                    task_type: "code_generation".into(),
                    criteria: SelectionCriteria::CheapestAboveQuality,
                    min_quality: 0.7,
                    max_cost_per_1k: Some(0.05),
                    max_latency_ms: Some(5000),
                    fallback: Some("gpt-4o".into()),
                },
                RoutingRule {
                    task_type: "*".into(),
                    criteria: SelectionCriteria::BestValue,
                    min_quality: 0.5,
                    max_cost_per_1k: None,
                    max_latency_ms: None,
                    fallback: None,
                },
            ],
            version: 1,
        };
        assert!(validate_policy(&policy).is_ok());
    }
}
