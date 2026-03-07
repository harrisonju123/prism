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
            let (quality, latency) = TIER_DEFAULTS[tier_idx];
            let cost = (info.input_cost_per_1m * 0.5 + info.output_cost_per_1m * 0.5) / 1000.0;

            for &task_type in TaskType::ALL_ROUTABLE {
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
        data.by_task_type = Self::build_index(&data.entries);
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
