use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::RwLock;

use super::types::FitnessEntry;
use crate::models::MODEL_CATALOG;
use crate::types::TaskType;

/// Default tier fitness values (quality, avg_latency_ms).
const TIER_DEFAULTS: [(f64, f64); 3] = [
    (0.93, 2000.0), // tier 1
    (0.79, 1000.0), // tier 2
    (0.56, 500.0),  // tier 3
];

#[derive(Clone)]
pub struct FitnessCache {
    data: Arc<RwLock<FitnessData>>,
    ttl_secs: u64,
    force_stale: Arc<AtomicBool>,
}

struct FitnessData {
    entries: Vec<FitnessEntry>,
    /// Pre-sorted by quality descending for each task type
    by_task_type: HashMap<TaskType, Vec<FitnessEntry>>,
    last_updated: Instant,
}

impl FitnessCache {
    pub fn new(ttl_secs: u64) -> Self {
        let synthetic = Self::build_synthetic_entries();
        let by_task_type = Self::build_index(&synthetic);
        Self {
            data: Arc::new(RwLock::new(FitnessData {
                entries: synthetic,
                by_task_type,
                last_updated: Instant::now(),
            })),
            ttl_secs,
            force_stale: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get fitness entries for a task type, sorted by quality descending.
    pub async fn get_entries_for_task(&self, task_type: TaskType) -> Vec<FitnessEntry> {
        let data = self.data.read().await;
        data.by_task_type
            .get(&task_type)
            .cloned()
            .unwrap_or_default()
    }

    /// Replace all entries and rebuild the index.
    pub async fn update(&self, entries: Vec<FitnessEntry>) {
        let by_task_type = Self::build_index(&entries);
        let mut data = self.data.write().await;
        data.entries = entries;
        data.by_task_type = by_task_type;
        data.last_updated = Instant::now();
        self.force_stale.store(false, Ordering::Relaxed);
    }

    /// Check if cache needs refresh.
    pub async fn is_stale(&self) -> bool {
        if self.force_stale.load(Ordering::Relaxed) {
            return true;
        }
        let data = self.data.read().await;
        data.last_updated.elapsed().as_secs() > self.ttl_secs
    }

    /// Force a cache refresh on next check.
    pub fn mark_stale(&self) {
        self.force_stale.store(true, Ordering::Relaxed);
    }

    /// Build synthetic fitness entries from the model catalog.
    fn build_synthetic_entries() -> Vec<FitnessEntry> {
        let mut entries = Vec::new();
        for (model_name, info) in MODEL_CATALOG.iter() {
            let tier_idx = (info.tier as usize).saturating_sub(1).min(2);
            let latency = TIER_DEFAULTS[tier_idx].1;
            let cost = (info.input_cost_per_1m * 0.5 + info.output_cost_per_1m * 0.5) / 1000.0;

            for &task_type in TaskType::ALL_ROUTABLE {
                let task_str = serde_json::to_value(task_type)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let quality = info.quality_for_task(&task_str);

                entries.push(FitnessEntry {
                    task_type,
                    model: model_name.to_string(),
                    avg_quality: quality,
                    avg_cost_per_1k: cost,
                    avg_latency_ms: latency,
                    sample_size: 0, // synthetic
                });
            }
        }
        entries
    }

    /// Update in-memory fitness for a single (task_type, model) pair from a benchmark result.
    pub async fn record_benchmark(
        &self,
        task_type: TaskType,
        model: &str,
        quality: f64,
        cost_per_1k: f64,
        latency_ms: f64,
    ) {
        let mut data = self.data.write().await;
        if let Some(entry) = data
            .entries
            .iter_mut()
            .find(|e| e.task_type == task_type && e.model == model)
        {
            let n = entry.sample_size as f64 + 1.0;
            entry.avg_quality += (quality - entry.avg_quality) / n;
            entry.avg_cost_per_1k += (cost_per_1k - entry.avg_cost_per_1k) / n;
            entry.avg_latency_ms += (latency_ms - entry.avg_latency_ms) / n;
            entry.sample_size += 1;
        } else {
            data.entries.push(FitnessEntry {
                task_type,
                model: model.to_string(),
                avg_quality: quality,
                avg_cost_per_1k: cost_per_1k,
                avg_latency_ms: latency_ms,
                sample_size: 1,
            });
        }
        // Rebuild only the affected task type's bucket rather than the full cross-product index
        let bucket: Vec<FitnessEntry> = data
            .entries
            .iter()
            .filter(|e| e.task_type == task_type)
            .cloned()
            .collect();
        let mut sorted = bucket;
        sorted.sort_by(|a, b| b.avg_quality.total_cmp(&a.avg_quality));
        data.by_task_type.insert(task_type, sorted);
        data.last_updated = std::time::Instant::now();
    }

    fn build_index(entries: &[FitnessEntry]) -> HashMap<TaskType, Vec<FitnessEntry>> {
        let mut map: HashMap<TaskType, Vec<FitnessEntry>> = HashMap::new();
        for entry in entries {
            map.entry(entry.task_type).or_default().push(entry.clone());
        }
        // Sort each list by quality descending
        for list in map.values_mut() {
            list.sort_by(|a, b| b.avg_quality.total_cmp(&a.avg_quality));
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn synthetic_entries_populated() {
        let cache = FitnessCache::new(300);
        // Each routable task type should have at least one entry
        for &tt in TaskType::ALL_ROUTABLE {
            let entries = cache.get_entries_for_task(tt).await;
            assert!(!entries.is_empty(), "no entry for task type {:?}", tt);
        }
    }

    #[tokio::test]
    async fn entries_sorted_by_quality_desc() {
        let cache = FitnessCache::new(300);
        for &tt in TaskType::ALL_ROUTABLE {
            let list = cache.get_entries_for_task(tt).await;
            if list.len() < 2 {
                continue;
            }
            for w in list.windows(2) {
                assert!(
                    w[0].avg_quality >= w[1].avg_quality,
                    "task {:?}: {} < {} out of order",
                    tt,
                    w[0].avg_quality,
                    w[1].avg_quality
                );
            }
        }
    }

    #[tokio::test]
    async fn update_replaces_all() {
        let cache = FitnessCache::new(300);
        let custom = vec![FitnessEntry {
            task_type: TaskType::Summarization,
            model: "custom-model".into(),
            avg_quality: 0.99,
            avg_cost_per_1k: 0.001,
            avg_latency_ms: 100.0,
            sample_size: 5,
        }];
        cache.update(custom).await;
        // After replacing, only the custom entry should exist for Summarization
        let entries = cache.get_entries_for_task(TaskType::Summarization).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model, "custom-model");
        // Other task types should have no entries
        let other = cache.get_entries_for_task(TaskType::CodeGeneration).await;
        assert!(other.is_empty());
    }

    #[tokio::test]
    async fn is_stale_fresh() {
        let cache = FitnessCache::new(3600);
        assert!(!cache.is_stale().await);
    }

    #[tokio::test]
    async fn mark_stale_forces_true() {
        let cache = FitnessCache::new(3600);
        cache.mark_stale();
        assert!(cache.is_stale().await);
    }

    #[tokio::test]
    async fn record_benchmark_existing() {
        let cache = FitnessCache::new(300);
        let entries_before = cache.get_entries_for_task(TaskType::CodeGeneration).await;
        let first = entries_before
            .into_iter()
            .next()
            .expect("should have CodeGeneration entry");

        let old_quality = first.avg_quality;
        cache
            .record_benchmark(TaskType::CodeGeneration, &first.model, 1.0, 0.01, 500.0)
            .await;

        let entries_after = cache.get_entries_for_task(TaskType::CodeGeneration).await;
        let updated = entries_after
            .iter()
            .find(|e| e.model == first.model)
            .expect("entry should still exist");
        // Running mean with quality=1.0 (above any synthetic score) → quality must increase
        assert!(
            updated.avg_quality > old_quality,
            "quality should increase after recording benchmark=1.0, was {old_quality}"
        );
        assert_eq!(updated.sample_size, first.sample_size + 1);
    }

    #[tokio::test]
    async fn record_benchmark_new() {
        let cache = FitnessCache::new(300);
        cache
            .record_benchmark(
                TaskType::Summarization,
                "brand-new-model",
                0.88,
                0.002,
                300.0,
            )
            .await;
        let entries = cache.get_entries_for_task(TaskType::Summarization).await;
        let new_entry = entries
            .iter()
            .find(|e| e.model == "brand-new-model")
            .expect("new entry should be created");
        assert_eq!(new_entry.sample_size, 1);
        assert!((new_entry.avg_quality - 0.88).abs() < 1e-9);
    }

    #[tokio::test]
    async fn record_benchmark_rebuilds_index() {
        let cache = FitnessCache::new(300);
        // Insert a perfect-quality entry for a new model
        cache
            .record_benchmark(TaskType::Reasoning, "perfect-model", 1.0, 0.0, 1.0)
            .await;
        let list = cache.get_entries_for_task(TaskType::Reasoning).await;
        // Existing synthetic entries should still be present
        assert!(list.len() > 1, "existing entries should be retained after recording new benchmark");
        // First entry should be the one with highest quality (perfect-model with q=1.0)
        assert_eq!(
            list[0].model, "perfect-model",
            "perfect-model should sort first"
        );
    }
}
