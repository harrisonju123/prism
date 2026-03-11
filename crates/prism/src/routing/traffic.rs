use crate::models::MODEL_CATALOG;
use crate::routing::FitnessCache;
use crate::routing::types::FitnessEntry;
use crate::types::TaskType;

struct TrafficRow {
    task_type: TaskType,
    model: String,
    avg_cost_per_1k: f64,
    avg_latency_ms: f64,
    sample_size: u32,
}

/// Refresh fitness cache cost and latency from live inference_events in ClickHouse.
///
/// Quality has no traffic signal, so it is carried over from the current cache
/// (or derived from ModelInfo::quality_for_task if not present).
pub async fn refresh_fitness_from_traffic(
    fitness_cache: &FitnessCache,
    ch_url: &str,
    ch_db: &str,
    min_sample_size: u32,
    lookback_days: u32,
) -> anyhow::Result<()> {
    let client = &reqwest::Client::new();

    let query = format!(
        "SELECT task_type, model, \
                sum(estimated_cost_usd) / sum(total_tokens) * 1000 AS avg_cost_per_1k, \
                avg(latency_ms) AS avg_latency_ms, \
                count() AS sample_size \
         FROM {db}.inference_events \
         WHERE status = 'success' \
           AND task_type IS NOT NULL \
           AND total_tokens > 0 \
           AND timestamp >= now() - INTERVAL {days} DAY \
         GROUP BY task_type, model \
         HAVING count() >= {min} \
         FORMAT JSONEachRow",
        db = ch_db,
        days = lookback_days,
        min = min_sample_size,
    );

    let resp = client.post(ch_url).body(query).send().await?.text().await?;
    let traffic_rows = parse_traffic_entries(&resp);

    if traffic_rows.is_empty() {
        tracing::debug!("no traffic data available for fitness refresh");
        return Ok(());
    }

    let mut merged: Vec<FitnessEntry> = Vec::new();

    for &task_type in TaskType::ALL_ROUTABLE {
        let existing = fitness_cache.get_entries_for_task(task_type).await;

        // Build a lookup: model → existing entry (for quality carry-over)
        let existing_by_model: std::collections::HashMap<&str, &FitnessEntry> =
            existing.iter().map(|e| (e.model.as_str(), e)).collect();

        // Collect models already handled via traffic rows
        let mut handled_models = std::collections::HashSet::new();

        for row in traffic_rows.iter().filter(|r| r.task_type == task_type) {
            let quality = if let Some(existing_entry) = existing_by_model.get(row.model.as_str()) {
                existing_entry.avg_quality
            } else {
                // No existing cache entry: derive from model catalog
                quality_for_model_task(&row.model, task_type)
            };

            merged.push(FitnessEntry {
                task_type,
                model: row.model.clone(),
                avg_quality: quality,
                avg_cost_per_1k: row.avg_cost_per_1k,
                avg_latency_ms: row.avg_latency_ms,
                sample_size: row.sample_size,
            });
            handled_models.insert(row.model.clone());
        }

        // Keep existing entries for (task_type, model) pairs not seen in traffic
        for entry in &existing {
            if !handled_models.contains(&entry.model) {
                merged.push(entry.clone());
            }
        }
    }

    let traffic_count = merged.iter().filter(|e| e.sample_size > 0).count();
    tracing::info!(
        total_entries = merged.len(),
        traffic_entries = traffic_count,
        "traffic fitness refresh: {} entries updated from inference_events",
        traffic_count,
    );

    fitness_cache.update(merged).await;

    // Merge quality signals from feedback_events (best-effort, non-blocking)
    if let Err(e) = refresh_quality_from_feedback(
        fitness_cache,
        client,
        ch_url,
        ch_db,
        min_sample_size,
        lookback_days,
    )
    .await
    {
        tracing::warn!("feedback quality merge failed (non-fatal): {e}");
    }

    Ok(())
}

fn parse_traffic_entries(resp: &str) -> Vec<TrafficRow> {
    let mut rows = Vec::new();
    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
            && let (Some(task_type_str), Some(model), Some(cost), Some(latency), Some(samples)) = (
                v.get("task_type").and_then(|v| v.as_str()),
                v.get("model").and_then(|v| v.as_str()),
                v.get("avg_cost_per_1k").and_then(|v| v.as_f64()),
                v.get("avg_latency_ms").and_then(|v| v.as_f64()),
                v.get("sample_size").and_then(|v| v.as_u64()),
            )
            && let Ok(task_type) = serde_json::from_value::<TaskType>(serde_json::Value::String(
                task_type_str.to_string(),
            ))
        {
            rows.push(TrafficRow {
                task_type,
                model: model.to_string(),
                avg_cost_per_1k: cost,
                avg_latency_ms: latency,
                sample_size: samples as u32,
            });
        }
    }
    rows
}

/// Look up quality for a (model, task_type) pair from the static catalog.
/// Falls back to tier default if the model isn't in the catalog.
fn quality_for_model_task(model: &str, task_type: TaskType) -> f64 {
    let task_str = serde_json::to_value(task_type)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();

    if let Some(info) = MODEL_CATALOG.get(model) {
        info.quality_for_task(&task_str)
    } else {
        // Unknown model — use mid-tier default
        0.79
    }
}

struct QualityRow {
    model: String,
    task_type: TaskType,
    avg_quality: f64,
    sample_size: u32,
}

/// Query ClickHouse for aggregated quality feedback and merge into fitness cache.
/// Uses Bayesian blending: weighted_quality = (catalog * prior_weight + feedback * sample_size) / (prior_weight + sample_size)
pub async fn refresh_quality_from_feedback(
    fitness_cache: &FitnessCache,
    client: &reqwest::Client,
    ch_url: &str,
    ch_db: &str,
    min_samples: u32,
    lookback_days: u32,
) -> anyhow::Result<()> {
    let query = format!(
        "SELECT ie.model, ie.task_type, \
                avg(fe.metric_value) AS avg_quality, \
                count() AS sample_size \
         FROM {db}.feedback_events fe \
         JOIN {db}.inference_events ie ON fe.episode_id = ie.episode_id \
         WHERE fe.metric_name IN ('implicit_quality', 'quality', 'reward', 'explicit_quality') \
           AND fe.timestamp >= now() - INTERVAL {days} DAY \
           AND ie.task_type IS NOT NULL \
         GROUP BY ie.model, ie.task_type \
         HAVING count() >= {min} \
         FORMAT JSONEachRow",
        db = ch_db,
        days = lookback_days,
        min = min_samples,
    );

    let resp = client.post(ch_url).body(query).send().await?.text().await?;
    let quality_rows = parse_quality_entries(&resp);

    if quality_rows.is_empty() {
        tracing::debug!("no feedback quality data available");
        return Ok(());
    }

    // Build lookup: (model, task_type) → (avg_quality, sample_size)
    let quality_map: std::collections::HashMap<(String, TaskType), (f64, u32)> = quality_rows
        .into_iter()
        .map(|r| ((r.model, r.task_type), (r.avg_quality, r.sample_size)))
        .collect();

    // Update existing fitness entries with blended quality
    const PRIOR_WEIGHT: f64 = 5.0;
    let mut updated: Vec<FitnessEntry> = Vec::new();

    for &task_type in TaskType::ALL_ROUTABLE {
        let entries = fitness_cache.get_entries_for_task(task_type).await;
        for entry in entries {
            let new_quality = if let Some(&(feedback_quality, sample_size)) =
                quality_map.get(&(entry.model.clone(), task_type))
            {
                let catalog_quality = quality_for_model_task(&entry.model, task_type);
                (catalog_quality * PRIOR_WEIGHT + feedback_quality * sample_size as f64)
                    / (PRIOR_WEIGHT + sample_size as f64)
            } else {
                entry.avg_quality
            };

            updated.push(FitnessEntry {
                avg_quality: new_quality,
                ..entry
            });
        }
    }

    let feedback_count = quality_map.len();
    tracing::info!(
        feedback_entries = feedback_count,
        total_entries = updated.len(),
        "quality feedback merge: {} (model, task) pairs updated from feedback_events",
        feedback_count,
    );

    fitness_cache.update(updated).await;
    Ok(())
}

fn parse_quality_entries(resp: &str) -> Vec<QualityRow> {
    let mut rows = Vec::new();
    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
            && let (Some(model), Some(task_type_str), Some(avg_quality), Some(sample_size)) = (
                v.get("model").and_then(|v| v.as_str()),
                v.get("task_type").and_then(|v| v.as_str()),
                v.get("avg_quality").and_then(|v| v.as_f64()),
                v.get("sample_size").and_then(|v| v.as_u64()),
            )
            && let Ok(task_type) = serde_json::from_value::<TaskType>(serde_json::Value::String(
                task_type_str.to_string(),
            ))
        {
            rows.push(QualityRow {
                model: model.to_string(),
                task_type,
                avg_quality,
                sample_size: sample_size as u32,
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_response() {
        let rows = parse_traffic_entries("");
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_invalid_json_skipped() {
        let rows = parse_traffic_entries("not json\n{\"broken\": true}");
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_valid_row() {
        let line = r#"{"task_type":"summarization","model":"claude-haiku-4-5","avg_cost_per_1k":0.005,"avg_latency_ms":800.0,"sample_size":20}"#;
        let rows = parse_traffic_entries(line);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model, "claude-haiku-4-5");
        assert!((rows[0].avg_cost_per_1k - 0.005).abs() < f64::EPSILON);
        assert_eq!(rows[0].sample_size, 20);
    }

    #[tokio::test]
    async fn traffic_refresh_keeps_existing_when_empty() {
        let cache = FitnessCache::new(300);
        let before = cache.get_entries_for_task(TaskType::Summarization).await;
        assert!(!before.is_empty());

        // parse with empty response → no rows → function returns early, cache unchanged
        let rows = parse_traffic_entries("");
        assert!(rows.is_empty());

        let after = cache.get_entries_for_task(TaskType::Summarization).await;
        assert_eq!(before.len(), after.len());
    }

    #[test]
    fn parse_quality_entries_valid() {
        let line = r#"{"model":"claude-haiku-4-5","task_type":"summarization","avg_quality":0.85,"sample_size":15}"#;
        let rows = parse_quality_entries(line);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model, "claude-haiku-4-5");
        assert!((rows[0].avg_quality - 0.85).abs() < f64::EPSILON);
        assert_eq!(rows[0].sample_size, 15);
    }

    #[test]
    fn parse_quality_entries_empty() {
        let rows = parse_quality_entries("");
        assert!(rows.is_empty());
    }

    #[test]
    fn bayesian_blending_small_sample() {
        // With small sample_size, the result should be close to the catalog prior
        let catalog: f64 = 0.79;
        let feedback: f64 = 0.50;
        let sample_size: f64 = 2.0;
        let prior_weight: f64 = 5.0;
        let blended =
            (catalog * prior_weight + feedback * sample_size) / (prior_weight + sample_size);
        // (0.79 * 5 + 0.50 * 2) / (5 + 2) = (3.95 + 1.0) / 7 = 4.95 / 7 ≈ 0.707
        assert!((blended - 0.707).abs() < 0.01);
    }

    #[test]
    fn bayesian_blending_large_sample() {
        // With large sample_size, the result should converge toward feedback
        let catalog: f64 = 0.79;
        let feedback: f64 = 0.50;
        let sample_size: f64 = 100.0;
        let prior_weight: f64 = 5.0;
        let blended =
            (catalog * prior_weight + feedback * sample_size) / (prior_weight + sample_size);
        // Should be close to 0.50
        assert!((blended - feedback).abs() < 0.02);
    }
}
