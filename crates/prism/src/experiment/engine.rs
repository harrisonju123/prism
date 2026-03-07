use std::collections::HashMap;

use dashmap::DashMap;
use uuid::Uuid;

use crate::config::{Experiment, ExperimentMode, Variant};

use super::bandit::BanditState;

/// Result of variant selection.
#[derive(Debug, Clone)]
pub struct VariantSelection {
    pub experiment_name: String,
    pub variant: Variant,
    pub episode_id: Uuid,
}

/// Manages experiment variant selection and episode stickiness.
pub struct ExperimentEngine {
    /// episode_id → (experiment_name → variant_name) for sticky assignments
    episode_assignments: DashMap<Uuid, HashMap<String, String>>,
    /// experiment_name → bandit state
    bandit_states: DashMap<String, BanditState>,
}

impl ExperimentEngine {
    pub fn new() -> Self {
        Self {
            episode_assignments: DashMap::new(),
            bandit_states: DashMap::new(),
        }
    }

    /// Select a variant for the given experiment and episode.
    pub fn select_variant(
        &self,
        experiment: &Experiment,
        experiment_name: &str,
        episode_id: Uuid,
    ) -> Option<VariantSelection> {
        if experiment.variants.is_empty() {
            return None;
        }

        // Check episode stickiness first
        if let Some(assignments) = self.episode_assignments.get(&episode_id)
            && let Some(variant_name) = assignments.get(experiment_name)
        {
            // Find the variant by name
            if let Some(variant) = experiment.variants.iter().find(|v| &v.name == variant_name) {
                return Some(VariantSelection {
                    experiment_name: experiment_name.to_string(),
                    variant: variant.clone(),
                    episode_id,
                });
            }
        }

        // Select variant based on mode
        let selected_name = match experiment.mode {
            ExperimentMode::Static => self.select_static(experiment),
            ExperimentMode::Bandit => self.select_bandit(experiment, experiment_name),
        };

        // Find the variant
        let variant = experiment
            .variants
            .iter()
            .find(|v| v.name == selected_name)?;

        // Record assignment for stickiness
        self.episode_assignments
            .entry(episode_id)
            .or_default()
            .insert(experiment_name.to_string(), selected_name);

        Some(VariantSelection {
            experiment_name: experiment_name.to_string(),
            variant: variant.clone(),
            episode_id,
        })
    }

    /// Record a reward for a specific arm.
    pub fn record_reward(&self, experiment_name: &str, variant_name: &str, reward: f64) {
        self.bandit_states
            .entry(experiment_name.to_string())
            .or_default()
            .update(variant_name, reward);
    }

    /// Look up all assignments for an episode and propagate reward to each.
    pub fn propagate_feedback(&self, episode_id: Uuid, reward: f64) {
        if let Some(assignments) = self.episode_assignments.get(&episode_id) {
            for (experiment_name, variant_name) in assignments.value() {
                self.record_reward(experiment_name, variant_name, reward);
            }
        }
    }

    /// Remove episode assignments older than 1 hour.
    /// Called periodically as a background task.
    pub fn prune_episodes(&self) {
        // Since we don't store timestamps on assignments, we prune all entries
        // that haven't been accessed recently. In practice, this just caps memory.
        // A simple approach: if the map is large, remove oldest entries.
        // For simplicity, we retain up to 100_000 entries by clearing entirely
        // when the map exceeds that threshold.
        if self.episode_assignments.len() > 100_000 {
            self.episode_assignments.clear();
            tracing::debug!("pruned episode assignments (exceeded 100k entries)");
        }
    }

    fn select_static(&self, experiment: &Experiment) -> String {
        if experiment.variants.len() == 1 {
            return experiment.variants[0].name.clone();
        }

        use rand::distr::Distribution;
        use rand::distr::weighted::WeightedIndex;

        let weights: Vec<f64> = experiment.variants.iter().map(|v| v.weight).collect();
        let dist = WeightedIndex::new(&weights).expect("non-empty weights");
        let mut rng = rand::rng();
        let idx = dist.sample(&mut rng);
        experiment.variants[idx].name.clone()
    }

    fn select_bandit(&self, experiment: &Experiment, experiment_name: &str) -> String {
        let variant_names: Vec<String> =
            experiment.variants.iter().map(|v| v.name.clone()).collect();

        let state = self
            .bandit_states
            .entry(experiment_name.to_string())
            .or_default();

        state.sample_best(&variant_names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Experiment, ExperimentMode, Variant};

    fn make_experiment(mode: ExperimentMode, variants: Vec<(&str, f64)>) -> Experiment {
        Experiment {
            function_name: "sonnet".to_string(),
            mode,
            variants: variants
                .into_iter()
                .map(|(name, weight)| Variant {
                    name: name.to_string(),
                    model: format!("model-{name}"),
                    weight,
                    temperature: None,
                    max_tokens: None,
                    system_prompt_prefix: None,
                })
                .collect(),
        }
    }

    #[test]
    fn static_weighted_selection() {
        let engine = ExperimentEngine::new();
        let exp = make_experiment(ExperimentMode::Static, vec![("a", 100.0), ("b", 0.001)]);

        let mut a_count = 0;
        for _ in 0..100 {
            let episode = Uuid::new_v4();
            let sel = engine.select_variant(&exp, "test", episode).unwrap();
            if sel.variant.name == "a" {
                a_count += 1;
            }
        }
        assert!(
            a_count > 80,
            "expected 'a' to win most often, got {a_count}/100"
        );
    }

    #[test]
    fn episode_stickiness() {
        let engine = ExperimentEngine::new();
        let exp = make_experiment(ExperimentMode::Static, vec![("a", 1.0), ("b", 1.0)]);

        let episode = Uuid::new_v4();
        let first = engine.select_variant(&exp, "test", episode).unwrap();
        let first_name = first.variant.name.clone();

        // Same episode should always return same variant
        for _ in 0..20 {
            let sel = engine.select_variant(&exp, "test", episode).unwrap();
            assert_eq!(sel.variant.name, first_name, "stickiness violated");
        }
    }

    #[test]
    fn different_episodes_can_differ() {
        let engine = ExperimentEngine::new();
        let exp = make_experiment(ExperimentMode::Static, vec![("a", 1.0), ("b", 1.0)]);

        let mut saw_different = false;
        let first_ep = Uuid::new_v4();
        let first = engine.select_variant(&exp, "test", first_ep).unwrap();

        for _ in 0..50 {
            let ep = Uuid::new_v4();
            let sel = engine.select_variant(&exp, "test", ep).unwrap();
            if sel.variant.name != first.variant.name {
                saw_different = true;
                break;
            }
        }
        assert!(
            saw_different,
            "50 different episodes all got same variant (extremely unlikely with 50/50 weights)"
        );
    }

    #[test]
    fn bandit_mode() {
        let engine = ExperimentEngine::new();
        let exp = make_experiment(ExperimentMode::Bandit, vec![("good", 1.0), ("bad", 1.0)]);

        // Give "good" many rewards
        for _ in 0..50 {
            engine.record_reward("test", "good", 1.0);
            engine.record_reward("test", "bad", 0.0);
        }

        // Now bandit should prefer "good"
        let mut good_count = 0;
        for _ in 0..100 {
            let ep = Uuid::new_v4();
            let sel = engine.select_variant(&exp, "test", ep).unwrap();
            if sel.variant.name == "good" {
                good_count += 1;
            }
        }
        assert!(
            good_count > 80,
            "expected 'good' to be selected >80/100 times, got {good_count}"
        );
    }

    #[test]
    fn single_variant() {
        let engine = ExperimentEngine::new();
        let exp = make_experiment(ExperimentMode::Static, vec![("only", 1.0)]);

        let ep = Uuid::new_v4();
        let sel = engine.select_variant(&exp, "test", ep).unwrap();
        assert_eq!(sel.variant.name, "only");
    }

    #[test]
    fn empty_variants() {
        let engine = ExperimentEngine::new();
        let exp = make_experiment(ExperimentMode::Static, vec![]);

        let ep = Uuid::new_v4();
        assert!(engine.select_variant(&exp, "test", ep).is_none());
    }

    #[test]
    fn reward_recording() {
        let engine = ExperimentEngine::new();
        engine.record_reward("exp1", "arm1", 1.0);
        engine.record_reward("exp1", "arm1", 0.0);

        let state = engine.bandit_states.get("exp1").unwrap();
        let arm = state.arms.get("arm1").unwrap();
        assert!((arm.alpha - 2.0).abs() < f64::EPSILON); // 1 + 1.0
        assert!((arm.beta - 2.0).abs() < f64::EPSILON); // 1 + 1.0
    }

    #[test]
    fn episode_pruning() {
        let engine = ExperimentEngine::new();

        // Insert entries below threshold — should not be pruned
        for _ in 0..10 {
            let ep = Uuid::new_v4();
            engine.episode_assignments.insert(ep, HashMap::new());
        }
        engine.prune_episodes();
        assert_eq!(engine.episode_assignments.len(), 10);
    }
}
