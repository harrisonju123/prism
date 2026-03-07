use super::fitness::FitnessCache;
use super::types::*;
use crate::models;
use crate::types::TaskType;

/// Resolve which model to use for a given request.
///
/// Steps:
/// 1. Empty policy -> passthrough (no routing)
/// 2. Find first matching rule by task_type or "*" catch-all
/// 3. Tier-1 preservation: if requested model is tier 1 + hard task + high confidence,
///    switch criteria to HighestQualityUnderCost, raise min_quality to 0.85
/// 4. Get candidates from fitness cache, filter by quality floor + constraints
/// 5. Apply criteria to sort and pick best
/// 6. Fallback if no candidate qualifies
pub async fn resolve(
    task_type: TaskType,
    confidence: f64,
    requested_model: &str,
    fitness_cache: &FitnessCache,
    policy: &RoutingPolicy,
    tier1_confidence_threshold: f64,
) -> Option<RoutingDecision> {
    // Step 1: empty policy = passthrough
    if policy.rules.is_empty() {
        return None;
    }

    // Step 2: find matching rule
    let task_str = serde_json::to_value(task_type)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "unknown".into());

    let (rule_idx, rule) = policy
        .rules
        .iter()
        .enumerate()
        .find(|(_, r)| r.task_type == task_str || r.task_type == "*")
        .map(|(i, r): (usize, &RoutingRule)| (i, r.clone()))?;

    // Step 3: tier-1 preservation
    let is_hard = HARD_TASKS.contains(&task_type);
    let requested_tier = models::lookup_model(requested_model)
        .map(|m| m.tier)
        .unwrap_or(2);

    let (criteria, min_quality) =
        if requested_tier == 1 && is_hard && confidence >= tier1_confidence_threshold {
            (
                SelectionCriteria::HighestQualityUnderCost,
                0.85_f64.max(rule.min_quality),
            )
        } else {
            (rule.criteria.clone(), rule.min_quality.max(QUALITY_FLOOR))
        };

    // Step 4: get candidates
    let candidates = fitness_cache.get_entries_for_task(task_type).await;
    let filtered: Vec<&FitnessEntry> = candidates
        .iter()
        .filter(|c| c.avg_quality >= min_quality)
        .filter(|c| {
            // Check tool support if needed
            if task_type == TaskType::ToolUse || task_type == TaskType::ToolSelection {
                models::lookup_model(&c.model)
                    .map(|m| m.supports_tools)
                    .unwrap_or(true)
            } else {
                true
            }
        })
        .filter(|c| {
            rule.max_cost_per_1k
                .map_or(true, |max| c.avg_cost_per_1k <= max)
        })
        .filter(|c| {
            rule.max_latency_ms
                .map_or(true, |max| c.avg_latency_ms <= max as f64)
        })
        .collect();

    // Step 5: sort by criteria and pick best
    let selected = pick_best(&criteria, &filtered);

    if let Some(entry) = selected {
        let was_overridden = entry.model != requested_model;
        Some(RoutingDecision {
            selected_model: entry.model.clone(),
            reason: format!(
                "criteria={:?}, quality={:.2}, cost={:.4}, latency={:.0}ms",
                criteria, entry.avg_quality, entry.avg_cost_per_1k, entry.avg_latency_ms
            ),
            was_overridden,
            policy_rule_id: Some(rule_idx),
            task_type,
            confidence,
        })
    } else {
        // Step 6: fallback
        rule.fallback.as_ref().map(|fb: &String| RoutingDecision {
            selected_model: fb.clone(),
            reason: "fallback: no candidate met constraints".into(),
            was_overridden: fb != requested_model,
            policy_rule_id: Some(rule_idx),
            task_type,
            confidence,
        })
    }
}

/// Replace NaN with NEG_INFINITY so NaN candidates lose in `max_by` comparisons.
#[inline]
fn nan_min(x: f64) -> f64 {
    if x.is_nan() { f64::NEG_INFINITY } else { x }
}

/// Pick the best candidate based on selection criteria.
fn pick_best<'a>(
    criteria: &SelectionCriteria,
    candidates: &[&'a FitnessEntry],
) -> Option<&'a FitnessEntry> {
    if candidates.is_empty() {
        return None;
    }

    match criteria {
        // NaN cost/latency sorts last in total_cmp → loses in min_by (correct).
        SelectionCriteria::CheapestAboveQuality => candidates
            .iter()
            .min_by(|a, b| a.avg_cost_per_1k.total_cmp(&b.avg_cost_per_1k))
            .copied(),
        SelectionCriteria::FastestAboveQuality => candidates
            .iter()
            .min_by(|a, b| a.avg_latency_ms.total_cmp(&b.avg_latency_ms))
            .copied(),
        // NaN quality must lose in max_by → map to NEG_INFINITY first.
        SelectionCriteria::HighestQualityUnderCost => candidates
            .iter()
            .max_by(|a, b| nan_min(a.avg_quality).total_cmp(&nan_min(b.avg_quality)))
            .copied(),
        SelectionCriteria::BestValue => {
            // Value = quality / cost (higher is better); NaN value loses.
            candidates
                .iter()
                .max_by(|a, b| {
                    let va = nan_min(a.avg_quality / a.avg_cost_per_1k.max(0.001));
                    let vb = nan_min(b.avg_quality / b.avg_cost_per_1k.max(0.001));
                    va.total_cmp(&vb)
                })
                .copied()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy() -> RoutingPolicy {
        RoutingPolicy {
            rules: vec![
                RoutingRule {
                    task_type: "code_generation".into(),
                    criteria: SelectionCriteria::CheapestAboveQuality,
                    min_quality: 0.70,
                    max_cost_per_1k: None,
                    max_latency_ms: None,
                    fallback: Some("claude-sonnet-4".into()),
                },
                RoutingRule {
                    task_type: "*".into(),
                    criteria: SelectionCriteria::CheapestAboveQuality,
                    min_quality: 0.55,
                    max_cost_per_1k: None,
                    max_latency_ms: None,
                    fallback: Some("claude-sonnet-4".into()),
                },
            ],
            version: 1,
        }
    }

    #[tokio::test]
    async fn resolve_routes_cheap_model_for_easy_task() {
        let cache = FitnessCache::new(300);
        let policy = test_policy();

        // Conversation is an easy task — should pick cheapest model above quality floor
        let decision = resolve(
            TaskType::Conversation,
            0.8,
            "claude-sonnet-4",
            &cache,
            &policy,
            0.7,
        )
        .await;

        assert!(decision.is_some(), "should produce a routing decision");
        let d = decision.unwrap();
        // With synthetic fitness, tier-3 models (quality 0.56) meet the 0.55 floor
        // and are cheaper than tier-2 models
        let tier1_models = ["claude-opus-4", "o1", "claude-3-opus", "gpt-4-turbo"];
        assert!(
            !tier1_models.contains(&d.selected_model.as_str()),
            "should not pick a tier-1 model for easy tasks, got: {}",
            d.selected_model
        );
    }

    #[tokio::test]
    async fn resolve_tier1_preservation() {
        let cache = FitnessCache::new(300);
        let policy = test_policy();

        // Request claude-opus-4 (tier 1) for code_generation (hard task) with high confidence
        let decision = resolve(
            TaskType::CodeGeneration,
            0.9,
            "claude-opus-4",
            &cache,
            &policy,
            0.7,
        )
        .await;

        assert!(decision.is_some());
        let d = decision.unwrap();
        // Tier-1 preservation should kick in: criteria becomes HighestQualityUnderCost
        // with min_quality 0.85 — only tier-1 models (0.93 quality) pass
        let tier1_models = ["claude-opus-4", "o1", "claude-3-opus", "gpt-4-turbo"];
        assert!(
            tier1_models.contains(&d.selected_model.as_str()),
            "should keep tier-1 model, got: {}",
            d.selected_model
        );
    }

    #[tokio::test]
    async fn resolve_empty_policy_passthrough() {
        let cache = FitnessCache::new(300);
        let policy = RoutingPolicy::default();

        let decision = resolve(
            TaskType::CodeGeneration,
            0.9,
            "claude-sonnet-4",
            &cache,
            &policy,
            0.7,
        )
        .await;

        assert!(decision.is_none(), "empty policy should pass through");
    }

    fn make_entry(model: &str, quality: f64, cost: f64, latency: f64) -> FitnessEntry {
        FitnessEntry {
            task_type: TaskType::Conversation,
            model: model.to_string(),
            avg_quality: quality,
            avg_cost_per_1k: cost,
            avg_latency_ms: latency,
            sample_size: 1,
        }
    }

    #[test]
    fn test_pick_best_nan_cost_does_not_panic() {
        let a = make_entry("good", 0.8, 1.0, 500.0);
        let b = make_entry("nan-cost", 0.8, f64::NAN, 500.0);
        let candidates = vec![&b, &a];
        let result = pick_best(&SelectionCriteria::CheapestAboveQuality, &candidates);
        assert!(result.is_some());
        assert_eq!(result.unwrap().model, "good");
    }

    #[test]
    fn test_pick_best_nan_quality_does_not_panic() {
        let a = make_entry("good", 0.8, 1.0, 500.0);
        let b = make_entry("nan-quality", f64::NAN, 1.0, 500.0);
        let candidates = vec![&b, &a];
        let result = pick_best(&SelectionCriteria::HighestQualityUnderCost, &candidates);
        assert!(result.is_some());
        assert_eq!(result.unwrap().model, "good");
    }

    #[test]
    fn test_pick_best_best_value_nan_does_not_panic() {
        let a = make_entry("good", 0.8, 1.0, 500.0);
        let b = make_entry("nan-both", f64::NAN, f64::NAN, 500.0);
        let candidates = vec![&b, &a];
        let result = pick_best(&SelectionCriteria::BestValue, &candidates);
        assert!(result.is_some());
        assert_eq!(result.unwrap().model, "good");
    }

    #[tokio::test]
    async fn resolve_fallback_on_tight_constraints() {
        let cache = FitnessCache::new(300);
        let policy = RoutingPolicy {
            rules: vec![RoutingRule {
                task_type: "*".into(),
                criteria: SelectionCriteria::CheapestAboveQuality,
                min_quality: 0.99, // impossibly high
                max_cost_per_1k: None,
                max_latency_ms: None,
                fallback: Some("claude-sonnet-4".into()),
            }],
            version: 1,
        };

        let decision = resolve(TaskType::Conversation, 0.8, "gpt-4o", &cache, &policy, 0.7).await;

        assert!(decision.is_some());
        let d = decision.unwrap();
        assert_eq!(d.selected_model, "claude-sonnet-4");
        assert!(d.reason.contains("fallback"));
    }
}
