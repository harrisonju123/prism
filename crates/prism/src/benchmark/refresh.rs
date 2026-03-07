use crate::routing::FitnessCache;
use crate::routing::types::FitnessEntry;
use crate::types::TaskType;

/// Refresh fitness cache from benchmark data in ClickHouse.
///
/// Queries for avg scores/cost/latency per (task_type, model) from benchmark_events
/// in the last 7 days. Merges with synthetic entries: real data replaces synthetic
/// where sample_size >= min_sample_size.
pub async fn refresh_fitness_from_benchmarks(
    fitness_cache: &FitnessCache,
    ch_url: &str,
    ch_db: &str,
    min_sample_size: u32,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    let query = format!(
        "SELECT task_type, benchmark_model as model, \
                avg(benchmark_score) as avg_quality, \
                avg(benchmark_cost) as avg_cost_per_1k, \
                avg(benchmark_latency_ms) as avg_latency_ms, \
                count() as sample_size \
         FROM {db}.benchmark_events \
         WHERE status = 'success' AND timestamp >= now() - INTERVAL 7 DAY \
         GROUP BY task_type, benchmark_model \
         FORMAT JSONEachRow",
        db = ch_db
    );

    let resp = client.post(ch_url).body(query).send().await?.text().await?;
    let real_entries = parse_benchmark_entries(&resp);

    if real_entries.is_empty() {
        tracing::debug!("no benchmark data available, keeping synthetic entries");
        return Ok(());
    }

    // Get current entries (includes synthetic)
    let mut merged = Vec::new();

    for &task_type in TaskType::ALL_ROUTABLE {
        let existing = fitness_cache.get_entries_for_task(task_type).await;

        for entry in existing {
            // Check if we have real data for this (task_type, model)
            if let Some(real) = real_entries
                .iter()
                .find(|r| r.task_type == task_type && r.model == entry.model)
            {
                if real.sample_size >= min_sample_size {
                    // Replace synthetic with real data
                    merged.push(real.clone());
                } else {
                    // Not enough samples yet, keep synthetic
                    merged.push(entry);
                }
            } else {
                // No real data, keep existing entry
                merged.push(entry);
            }
        }

        // Also add any real entries for models not in the existing set
        for real in &real_entries {
            if real.task_type == task_type
                && real.sample_size >= min_sample_size
                && !merged
                    .iter()
                    .any(|m| m.task_type == task_type && m.model == real.model)
            {
                merged.push(real.clone());
            }
        }
    }

    let real_count = merged.iter().filter(|e| e.sample_size > 0).count();
    tracing::info!(
        total_entries = merged.len(),
        real_data_entries = real_count,
        "fitness cache refreshed from benchmarks"
    );

    fitness_cache.update(merged).await;
    Ok(())
}

fn parse_benchmark_entries(resp: &str) -> Vec<FitnessEntry> {
    let mut entries = Vec::new();
    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
            && let (
                Some(task_type_str),
                Some(model),
                Some(quality),
                Some(cost),
                Some(latency),
                Some(samples),
            ) = (
                v.get("task_type").and_then(|v| v.as_str()),
                v.get("model").and_then(|v| v.as_str()),
                v.get("avg_quality").and_then(|v| v.as_f64()),
                v.get("avg_cost_per_1k").and_then(|v| v.as_f64()),
                v.get("avg_latency_ms").and_then(|v| v.as_f64()),
                v.get("sample_size").and_then(|v| v.as_u64()),
            )
            && let Ok(task_type) = serde_json::from_value::<TaskType>(serde_json::Value::String(
                task_type_str.to_string(),
            ))
        {
            entries.push(FitnessEntry {
                task_type,
                model: model.to_string(),
                avg_quality: quality,
                avg_cost_per_1k: cost,
                avg_latency_ms: latency,
                sample_size: samples as u32,
            });
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn merge_real_replaces_synthetic() {
        let cache = FitnessCache::new(300);

        // Cache starts with synthetic entries (sample_size=0)
        let synthetic = cache.get_entries_for_task(TaskType::Summarization).await;
        assert!(!synthetic.is_empty());
        assert!(synthetic.iter().all(|e| e.sample_size == 0));

        // Simulate merging real data
        let mut entries = synthetic.clone();
        let real = FitnessEntry {
            task_type: TaskType::Summarization,
            model: entries[0].model.clone(),
            avg_quality: 0.88,
            avg_cost_per_1k: 0.005,
            avg_latency_ms: 800.0,
            sample_size: 50,
        };

        // Replace the first entry with real data
        entries[0] = real.clone();
        cache.update(entries).await;

        let updated = cache.get_entries_for_task(TaskType::Summarization).await;
        let matched = updated.iter().find(|e| e.model == real.model).unwrap();
        assert_eq!(matched.sample_size, 50);
        assert!((matched.avg_quality - 0.88).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn empty_benchmark_data_keeps_synthetic() {
        let cache = FitnessCache::new(300);

        let before = cache.get_entries_for_task(TaskType::CodeGeneration).await;
        assert!(!before.is_empty());

        // parse_benchmark_entries with empty response returns nothing
        let entries = parse_benchmark_entries("");
        assert!(entries.is_empty());

        // Verify cache is unchanged
        let after = cache.get_entries_for_task(TaskType::CodeGeneration).await;
        assert_eq!(before.len(), after.len());
    }
}
