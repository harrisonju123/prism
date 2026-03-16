use sqlx::SqlitePool;

use crate::config::WasteConfig;
use crate::routing::FitnessCache;
use crate::types::TaskType;

use super::{
    OverkillEntry, OverspendEntry, WasteCategory, WasteItem, WasteReport, WasteSeverity,
    estimate_input_cost_fraction, model_tier,
};

const SYSTEM_PROMPT_MIN_TOKENS: i64 = 2000;
const COMPLETION_MAX_TOKENS: i64 = 100;
const TRIM_FACTOR: f64 = 0.5;
const MIN_LOOP_ITERATIONS: i64 = 3;

/// Generate a waste report from the local SQLite inference_events store.
pub async fn generate_waste_report_local(
    pool: &SqlitePool,
    fitness_cache: &FitnessCache,
    waste_config: &WasteConfig,
    period_days: u32,
) -> anyhow::Result<WasteReport> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(period_days as i64);
    let cutoff_str = cutoff.to_rfc3339();

    // --- Totals ---
    let row = sqlx::query_as::<_, (i64, f64)>(
        "SELECT COUNT(*) as total_requests, COALESCE(SUM(estimated_cost_usd), 0.0) as total_cost \
         FROM inference_events WHERE timestamp >= ?",
    )
    .bind(&cutoff_str)
    .fetch_one(pool)
    .await?;
    let total_requests = row.0 as u64;
    let total_cost_usd = row.1;

    // --- Usage data per (task_type, model) ---
    let usage_rows = sqlx::query_as::<_, (Option<String>, String, i64)>(
        "SELECT task_type, model, COUNT(*) as request_count \
         FROM inference_events \
         WHERE timestamp >= ? AND task_type IS NOT NULL \
         GROUP BY task_type, model",
    )
    .bind(&cutoff_str)
    .fetch_all(pool)
    .await?;
    let usage_data: Vec<UsageRow> = usage_rows
        .into_iter()
        .map(|(task_type, model, count)| UsageRow {
            task_type: task_type.unwrap_or_default(),
            model,
            request_count: count as u64,
        })
        .collect();

    // --- Detectors ---
    let overkill = detect_overkill_local(fitness_cache, waste_config, &usage_data).await;
    let overspend = detect_overspend_local(pool, &cutoff_str, waste_config).await?;

    let mut items = Vec::new();
    items.extend(detect_redundant_calls_local(pool, &cutoff_str).await?);
    items.extend(detect_cache_misses_local(pool, &cutoff_str).await?);
    items.extend(detect_context_bloat_local(pool, &cutoff_str).await?);
    items.extend(detect_agent_loops_local(pool, &cutoff_str).await?);

    let estimated_waste: f64 = overkill.iter().map(|e| e.wasted_cost_usd).sum::<f64>()
        + overspend.iter().map(|e| e.total_overspend_usd).sum::<f64>()
        + items.iter().map(|i| i.savings).sum::<f64>();
    let waste_percentage = if total_cost_usd > 0.0 {
        (estimated_waste / total_cost_usd * 100.0).min(100.0)
    } else {
        0.0
    };

    Ok(WasteReport {
        generated_at: chrono::Utc::now().to_rfc3339(),
        period_days,
        total_requests,
        total_cost_usd,
        estimated_waste_usd: estimated_waste,
        waste_percentage,
        overkill,
        overspend,
        items,
    })
}

struct UsageRow {
    task_type: String,
    model: String,
    request_count: u64,
}

// ---------------------------------------------------------------------------
// Overkill (mirrors detector.rs approach — uses FitnessCache entries)
// ---------------------------------------------------------------------------

async fn detect_overkill_local(
    fitness_cache: &FitnessCache,
    config: &WasteConfig,
    usage_data: &[UsageRow],
) -> Vec<OverkillEntry> {
    let mut results = Vec::new();

    for &task_type in TaskType::ALL_ROUTABLE {
        let entries = fitness_cache.get_entries_for_task(task_type).await;
        let task_str = task_type.to_string();

        for expensive in &entries {
            let expensive_tier = model_tier(&expensive.model);
            if expensive_tier > 2 || expensive.sample_size == 0 {
                continue;
            }

            for cheaper in &entries {
                if cheaper.model == expensive.model {
                    continue;
                }
                let quality_diff = expensive.avg_quality - cheaper.avg_quality;
                if quality_diff > config.quality_tolerance {
                    continue;
                }
                if cheaper.avg_cost_per_1k >= config.cost_ratio_threshold * expensive.avg_cost_per_1k {
                    continue;
                }

                let request_count = usage_data
                    .iter()
                    .find(|u| u.task_type == task_str && u.model == expensive.model)
                    .map(|u| u.request_count)
                    .unwrap_or(0);
                if request_count == 0 {
                    continue;
                }

                let wasted_cost = (expensive.avg_cost_per_1k - cheaper.avg_cost_per_1k)
                    * request_count as f64
                    / 1000.0;

                results.push(OverkillEntry {
                    task_type: task_str.clone(),
                    expensive_model: expensive.model.clone(),
                    expensive_model_tier: expensive_tier,
                    expensive_model_score: expensive.avg_quality,
                    cheaper_alternative: cheaper.model.clone(),
                    cheaper_model_tier: model_tier(&cheaper.model),
                    cheaper_model_score: cheaper.avg_quality,
                    request_count,
                    wasted_cost_usd: wasted_cost,
                    recommendation: format!(
                        "For {task_str}, consider switching from {} to {} \
                         for similar quality at lower cost.",
                        expensive.model, cheaper.model
                    ),
                });
                break;
            }
        }
    }

    results.sort_by(|a, b| {
        b.wasted_cost_usd
            .partial_cmp(&a.wasted_cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}


// ---------------------------------------------------------------------------
// Overspend
// ---------------------------------------------------------------------------

async fn detect_overspend_local(
    pool: &SqlitePool,
    cutoff_str: &str,
    waste_config: &WasteConfig,
) -> anyhow::Result<Vec<OverspendEntry>> {
    let rows = sqlx::query_as::<_, (Option<String>, String, i64, f64)>(
        "SELECT task_type, model, COUNT(*) as request_count, \
                AVG(estimated_cost_usd) as avg_cost \
         FROM inference_events \
         WHERE timestamp >= ? AND task_type IS NOT NULL \
         GROUP BY task_type, model \
         HAVING request_count >= 5",
    )
    .bind(cutoff_str)
    .fetch_all(pool)
    .await?;

    // Compute median per task_type
    let mut by_task: std::collections::HashMap<String, Vec<f64>> = std::collections::HashMap::new();
    for (task_type, _, _, avg_cost) in &rows {
        if let Some(t) = task_type {
            by_task.entry(t.clone()).or_default().push(*avg_cost);
        }
    }
    let medians: std::collections::HashMap<String, f64> = by_task
        .into_iter()
        .map(|(k, mut v)| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = if v.is_empty() {
                0.0
            } else {
                let mid = v.len() / 2;
                if v.len() % 2 == 0 {
                    (v[mid - 1] + v[mid]) / 2.0
                } else {
                    v[mid]
                }
            };
            (k, median)
        })
        .collect();

    let mut overspend = Vec::new();
    for (task_type, model, request_count, avg_cost) in rows {
        let task_type = match task_type {
            Some(t) => t,
            None => continue,
        };
        let median = *medians.get(&task_type).unwrap_or(&0.0);
        if median <= 0.0 {
            continue;
        }
        let overspend_factor = avg_cost / median;
        if overspend_factor < waste_config.overspend_multiplier {
            continue;
        }
        let count = request_count as u64;
        let total_overspend = count as f64 * (avg_cost - median);
        overspend.push(OverspendEntry {
            task_type,
            model,
            request_count: count,
            median_cost: median,
            flagged_cost: avg_cost,
            overspend_factor,
            total_overspend_usd: total_overspend,
        });
    }

    overspend.sort_by(|a, b| {
        b.total_overspend_usd
            .partial_cmp(&a.total_overspend_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(overspend)
}

// ---------------------------------------------------------------------------
// Redundant calls
// ---------------------------------------------------------------------------

async fn detect_redundant_calls_local(
    pool: &SqlitePool,
    cutoff_str: &str,
) -> anyhow::Result<Vec<WasteItem>> {
    let rows = sqlx::query_as::<_, (String, i64, f64, Option<String>)>(
        "SELECT prompt_hash, COUNT(*) as dup_count, \
                AVG(estimated_cost_usd) as avg_cost, \
                MAX(trace_id) as trace_id \
         FROM inference_events \
         WHERE timestamp >= ? AND prompt_hash != '' \
         GROUP BY prompt_hash \
         HAVING dup_count >= 2 \
         ORDER BY dup_count DESC \
         LIMIT 100",
    )
    .bind(cutoff_str)
    .fetch_all(pool)
    .await?;

    let mut items = Vec::new();
    for (prompt_hash, dup_count, avg_cost, trace_id) in rows {
        let dup_count = dup_count as u64;
        if dup_count < 2 {
            continue;
        }
        let wasted_calls = dup_count - 1;
        let savings = wasted_calls as f64 * avg_cost;
        let severity = if dup_count >= 10 {
            WasteSeverity::Critical
        } else if dup_count >= 5 {
            WasteSeverity::Warning
        } else {
            WasteSeverity::Info
        };
        let prefix = &prompt_hash[..prompt_hash.len().min(12)];
        items.push(WasteItem {
            category: WasteCategory::RedundantCalls,
            severity,
            affected_trace_ids: trace_id.into_iter().collect(),
            call_count: dup_count,
            current_cost: dup_count as f64 * avg_cost,
            projected_cost: avg_cost,
            savings,
            description: format!(
                "Prompt {prefix}... sent {dup_count}x — {wasted_calls} redundant calls"
            ),
            confidence: 0.95,
        });
    }
    items.sort_by(|a, b| b.savings.partial_cmp(&a.savings).unwrap());
    Ok(items)
}

// ---------------------------------------------------------------------------
// Cache misses
// ---------------------------------------------------------------------------

async fn detect_cache_misses_local(
    pool: &SqlitePool,
    cutoff_str: &str,
) -> anyhow::Result<Vec<WasteItem>> {
    let rows = sqlx::query_as::<_, (String, i64, f64)>(
        "SELECT prompt_hash, COUNT(*) as dup_count, \
                SUM(estimated_cost_usd) as total_cost \
         FROM inference_events \
         WHERE timestamp >= ? AND prompt_hash != '' AND cache_read_input_tokens = 0 \
         GROUP BY prompt_hash \
         HAVING dup_count >= 3 \
         ORDER BY total_cost DESC \
         LIMIT 100",
    )
    .bind(cutoff_str)
    .fetch_all(pool)
    .await?;

    let mut items = Vec::new();
    for (prompt_hash, dup_count, total_cost) in rows {
        let dup_count = dup_count as u64;
        if dup_count < 3 {
            continue;
        }
        let cost_per_call = total_cost / dup_count as f64;
        let cached_cost = cost_per_call * 0.1;
        let wasted_calls = dup_count - 1;
        let savings = wasted_calls as f64 * (cost_per_call - cached_cost);
        if savings <= 0.0 {
            continue;
        }
        let severity = if savings >= 100.0 || dup_count >= 50 {
            WasteSeverity::Critical
        } else if savings >= 10.0 || dup_count >= 10 {
            WasteSeverity::Warning
        } else {
            WasteSeverity::Info
        };
        let prefix = &prompt_hash[..prompt_hash.len().min(12)];
        items.push(WasteItem {
            category: WasteCategory::CacheMisses,
            severity,
            affected_trace_ids: vec![],
            call_count: dup_count,
            current_cost: total_cost,
            projected_cost: cost_per_call + (wasted_calls as f64 * cached_cost),
            savings,
            description: format!(
                "Prompt {prefix}... sent {dup_count}x without caching — \
                 {wasted_calls} calls could use prompt caching"
            ),
            confidence: 0.9,
        });
    }
    items.sort_by(|a, b| b.savings.partial_cmp(&a.savings).unwrap());
    Ok(items)
}

// ---------------------------------------------------------------------------
// Context bloat
// ---------------------------------------------------------------------------

async fn detect_context_bloat_local(
    pool: &SqlitePool,
    cutoff_str: &str,
) -> anyhow::Result<Vec<WasteItem>> {
    let rows = sqlx::query_as::<_, (Option<String>, String, i64, i64, f64, i64)>(
        "SELECT trace_id, model, \
                SUM(input_tokens) as total_input, \
                SUM(output_tokens) as total_output, \
                SUM(estimated_cost_usd) as total_cost, \
                COUNT(*) as event_count \
         FROM inference_events \
         WHERE timestamp >= ? AND input_tokens >= ? AND output_tokens <= ? \
         GROUP BY trace_id, model \
         ORDER BY total_cost DESC \
         LIMIT 100",
    )
    .bind(cutoff_str)
    .bind(SYSTEM_PROMPT_MIN_TOKENS)
    .bind(COMPLETION_MAX_TOKENS)
    .fetch_all(pool)
    .await?;

    let mut items = Vec::new();
    for (trace_id, model, total_input, total_output, total_cost, event_count) in rows {
        if total_input == 0 {
            continue;
        }
        let trimmed = total_input as f64 * TRIM_FACTOR;
        let savings_fraction = trimmed / total_input as f64;
        let input_cost_fraction = estimate_input_cost_fraction(&model);
        let savings = total_cost * savings_fraction * input_cost_fraction;
        if savings <= 0.0 {
            continue;
        }
        let severity = if total_input > 10_000 && event_count >= 10 {
            WasteSeverity::Critical
        } else if total_input > 5_000 || event_count >= 5 {
            WasteSeverity::Warning
        } else {
            WasteSeverity::Info
        };
        items.push(WasteItem {
            category: WasteCategory::ContextBloat,
            severity,
            affected_trace_ids: trace_id.into_iter().collect(),
            call_count: event_count as u64,
            current_cost: total_cost,
            projected_cost: total_cost - savings,
            savings,
            description: format!(
                "{event_count} calls with {total_input} input tokens but only \
                 {total_output} output tokens — prompt may be bloated"
            ),
            confidence: 0.7,
        });
    }
    items.sort_by(|a, b| b.savings.partial_cmp(&a.savings).unwrap());
    Ok(items)
}


// ---------------------------------------------------------------------------
// Agent loops
// ---------------------------------------------------------------------------

async fn detect_agent_loops_local(
    pool: &SqlitePool,
    cutoff_str: &str,
) -> anyhow::Result<Vec<WasteItem>> {
    // Count distinct completion_hash per trace as a proxy for unique completions
    let rows = sqlx::query_as::<_, (String, i64, f64, i64)>(
        "SELECT trace_id, \
                COUNT(*) as call_count, \
                SUM(estimated_cost_usd) as total_cost, \
                COUNT(DISTINCT completion_hash) as unique_completions \
         FROM inference_events \
         WHERE timestamp >= ? \
           AND trace_id IS NOT NULL AND trace_id != '' \
           AND tool_calls_json IS NOT NULL \
         GROUP BY trace_id \
         HAVING call_count >= ? AND unique_completions < call_count / 2 \
         ORDER BY total_cost DESC \
         LIMIT 100",
    )
    .bind(cutoff_str)
    .bind(MIN_LOOP_ITERATIONS)
    .fetch_all(pool)
    .await?;

    let mut items = Vec::new();
    for (trace_id, call_count, total_cost, unique_completions) in rows {
        let call_count = call_count as u64;
        if call_count < MIN_LOOP_ITERATIONS as u64 {
            continue;
        }
        let productive_calls = 2u64.min(call_count);
        let wasted_calls = call_count - productive_calls;
        let cost_per_call = if call_count > 0 {
            total_cost / call_count as f64
        } else {
            0.0
        };
        let savings = wasted_calls as f64 * cost_per_call;
        if savings <= 0.0 {
            continue;
        }
        let loop_length = call_count - unique_completions as u64;
        let severity = if loop_length >= 10 {
            WasteSeverity::Critical
        } else if loop_length >= 5 {
            WasteSeverity::Warning
        } else {
            WasteSeverity::Info
        };
        items.push(WasteItem {
            category: WasteCategory::AgentLoops,
            severity,
            affected_trace_ids: vec![trace_id],
            call_count,
            current_cost: total_cost,
            projected_cost: total_cost - savings,
            savings,
            description: format!(
                "Trace has {call_count} calls but only {unique_completions} unique completions — \
                 possible fix-break-fix loop"
            ),
            confidence: 0.8,
        });
    }
    items.sort_by(|a, b| b.savings.partial_cmp(&a.savings).unwrap());
    Ok(items)
}
