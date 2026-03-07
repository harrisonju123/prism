use std::collections::HashMap;

use crate::routing::FitnessCache;
use crate::types::TaskType;

/// EMA-based feedback adjustment for the fitness matrix.
///
/// Feedback is the third merge stage: synthetic -> benchmark -> feedback.
/// Adjustments are clamped and floored to prevent feedback from degrading
/// quality below the synthetic baseline.

/// State for tracking EMA values across adjustment cycles.
#[derive(Debug, Default)]
pub struct FeedbackAdjusterState {
    ema: HashMap<(String, String), f64>,
}

/// A single feedback row from ClickHouse aggregation.
#[derive(Debug)]
pub struct FeedbackRow {
    pub model: String,
    pub task_type: String,
    pub avg_delta: f64,
    pub sample_count: u32,
}

/// Compute per-(model, task_type) quality adjustments from feedback.
///
/// Each feedback row has: model, task_type, avg_delta (weighted avg
/// of quality_delta), sample_count.
///
/// Returns dict mapping (model, task_type) -> adjustment value.
pub fn compute_adjustments(
    feedback_rows: &[FeedbackRow],
    state: &mut FeedbackAdjusterState,
    alpha: f64,
    min_samples: u32,
    max_adjustment: f64,
) -> HashMap<(String, String), f64> {
    let mut adjustments = HashMap::new();

    for row in feedback_rows {
        if row.sample_count < min_samples {
            continue;
        }

        let key = (row.model.clone(), row.task_type.clone());

        // EMA: new_value = alpha * observation + (1 - alpha) * previous
        let prev = state.ema.get(&key).copied().unwrap_or(0.0);
        let smoothed = alpha * row.avg_delta + (1.0 - alpha) * prev;
        state.ema.insert(key.clone(), smoothed);

        // Clamp to [-max_adjustment, +max_adjustment]
        let clamped = smoothed.clamp(-max_adjustment, max_adjustment);
        adjustments.insert(key, clamped);
    }

    adjustments
}

/// Apply feedback adjustments to fitness cache.
///
/// Adjusted quality never drops below the synthetic floor for that
/// (model, task_type) pair — prevents feedback spirals from tanking a model.
pub async fn apply_adjustments(
    fitness_cache: &FitnessCache,
    adjustments: &HashMap<(String, String), f64>,
) {
    if adjustments.is_empty() {
        return;
    }

    // Collect current entries with adjustments applied
    let mut adjusted_entries = Vec::new();
    for &task_type in TaskType::ALL_ROUTABLE {
        let entries = fitness_cache.get_entries_for_task(task_type).await;
        for mut entry in entries {
            let key = (entry.model.clone(), task_type.to_string());
            if let Some(&adj) = adjustments.get(&key) {
                // Apply adjustment with floor at synthetic default
                let synthetic_floor = synthetic_quality_floor(entry.sample_size);
                entry.avg_quality = (entry.avg_quality + adj).clamp(synthetic_floor, 1.0);
            }
            adjusted_entries.push(entry);
        }
    }

    fitness_cache.update(adjusted_entries).await;
}

/// Synthetic floor: if sample_size == 0 (synthetic), the quality is the floor.
/// For real data, use a conservative floor.
fn synthetic_quality_floor(sample_size: u32) -> f64 {
    if sample_size == 0 {
        0.0 // synthetic entries self-define their floor
    } else {
        0.3 // absolute minimum
    }
}

/// Query ClickHouse for aggregated feedback deltas per (model, task_type).
pub async fn query_feedback_deltas(
    ch_url: &str,
    ch_db: &str,
    min_samples: u32,
) -> anyhow::Result<Vec<FeedbackRow>> {
    let client = reqwest::Client::new();

    // Join feedback_events with inference_events to get model + task_type
    // metric_name = 'quality' gives us the quality delta
    let query = format!(
        "SELECT ie.model as model, \
                ie.task_type as task_type, \
                avg(fe.metric_value - 0.5) as avg_delta, \
                count() as sample_count \
         FROM {db}.feedback_events fe \
         INNER JOIN {db}.inference_events ie ON fe.inference_id = ie.id \
         WHERE fe.timestamp >= now() - INTERVAL 7 DAY \
           AND fe.metric_name = 'quality' \
           AND ie.task_type IS NOT NULL \
         GROUP BY ie.model, ie.task_type \
         HAVING sample_count >= {min_samples} \
         FORMAT JSONEachRow",
        db = ch_db,
        min_samples = min_samples,
    );

    let resp = client.post(ch_url).body(query).send().await?.text().await?;

    let mut rows = Vec::new();
    for line in resp.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let model = v
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let task_type = v
                .get("task_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let avg_delta = v.get("avg_delta").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sample_count = v.get("sample_count").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

            if !model.is_empty() && !task_type.is_empty() {
                rows.push(FeedbackRow {
                    model,
                    task_type,
                    avg_delta,
                    sample_count,
                });
            }
        }
    }

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_adjustments_basic() {
        let rows = vec![
            FeedbackRow {
                model: "gpt-4o".into(),
                task_type: "code_generation".into(),
                avg_delta: 0.1,
                sample_count: 30,
            },
            FeedbackRow {
                model: "claude-sonnet-4".into(),
                task_type: "summarization".into(),
                avg_delta: -0.2,
                sample_count: 25,
            },
        ];

        let mut state = FeedbackAdjusterState::default();
        let adjustments = compute_adjustments(&rows, &mut state, 0.05, 20, 0.15);

        assert_eq!(adjustments.len(), 2);

        let gpt4_adj = adjustments
            .get(&("gpt-4o".into(), "code_generation".into()))
            .unwrap();
        // EMA: 0.05 * 0.1 + 0.95 * 0.0 = 0.005
        assert!((*gpt4_adj - 0.005).abs() < 1e-9);

        let sonnet_adj = adjustments
            .get(&("claude-sonnet-4".into(), "summarization".into()))
            .unwrap();
        // EMA: 0.05 * -0.2 + 0.95 * 0.0 = -0.01
        assert!((*sonnet_adj - (-0.01)).abs() < 1e-9);
    }

    #[test]
    fn compute_adjustments_skips_low_samples() {
        let rows = vec![FeedbackRow {
            model: "gpt-4o".into(),
            task_type: "code_generation".into(),
            avg_delta: 0.5,
            sample_count: 5,
        }];

        let mut state = FeedbackAdjusterState::default();
        let adjustments = compute_adjustments(&rows, &mut state, 0.05, 20, 0.15);

        assert!(adjustments.is_empty());
    }

    #[test]
    fn compute_adjustments_clamps_to_max() {
        let rows = vec![FeedbackRow {
            model: "gpt-4o".into(),
            task_type: "code_generation".into(),
            avg_delta: 5.0, // extreme
            sample_count: 100,
        }];

        let mut state = FeedbackAdjusterState::default();
        let adjustments = compute_adjustments(&rows, &mut state, 1.0, 1, 0.15);

        let adj = adjustments
            .get(&("gpt-4o".into(), "code_generation".into()))
            .unwrap();
        assert!((*adj - 0.15).abs() < 1e-9, "should clamp to max_adjustment");
    }

    #[test]
    fn ema_accumulates_across_calls() {
        let rows = vec![FeedbackRow {
            model: "gpt-4o".into(),
            task_type: "code_generation".into(),
            avg_delta: 0.1,
            sample_count: 30,
        }];

        let mut state = FeedbackAdjusterState::default();

        // First call: EMA = 0.05 * 0.1 = 0.005
        let adj1 = compute_adjustments(&rows, &mut state, 0.05, 20, 0.15);
        let v1 = adj1
            .get(&("gpt-4o".into(), "code_generation".into()))
            .unwrap();
        assert!((*v1 - 0.005).abs() < 1e-9);

        // Second call: EMA = 0.05 * 0.1 + 0.95 * 0.005 = 0.005 + 0.00475 = 0.00975
        let adj2 = compute_adjustments(&rows, &mut state, 0.05, 20, 0.15);
        let v2 = adj2
            .get(&("gpt-4o".into(), "code_generation".into()))
            .unwrap();
        assert!((*v2 - 0.00975).abs() < 1e-9);
    }
}
