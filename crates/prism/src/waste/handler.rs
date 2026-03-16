use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;

#[derive(Debug, Deserialize)]
pub struct WasteReportParams {
    #[serde(default = "default_period_days")]
    pub period_days: u32,
}

fn default_period_days() -> u32 {
    7
}

fn ensure_waste_enabled(state: &AppState) -> Result<()> {
    if !state.config.waste.enabled {
        Err(PrismError::BadRequest(
            "waste detection is disabled".to_string(),
        ))
    } else {
        Ok(())
    }
}

async fn generate_report(
    state: &AppState,
    period_days: u32,
) -> Result<super::WasteReport> {
    // Prefer ClickHouse when available (full build + configured)
    #[cfg(feature = "full")]
    {
        if !state.config.clickhouse.url.is_empty() {
            return super::detector::generate_waste_report(
                &state.config.clickhouse.url,
                &state.config.clickhouse.database,
                &state.fitness_cache,
                &state.config.waste,
                period_days,
            )
            .await
            .map_err(|e| PrismError::Internal(format!("waste report generation failed: {e}")));
        }
    }

    // Fall back to local SQLite store
    let writer = state.local_inference_writer.as_ref().ok_or_else(|| {
        PrismError::BadRequest(
            "no observability backend available — start gateway with a local observability store \
             or configure ClickHouse"
                .to_string(),
        )
    })?;

    super::local_detector::generate_waste_report_local(
        writer.pool(),
        &state.fitness_cache,
        &state.config.waste,
        period_days,
    )
    .await
    .map_err(|e| PrismError::Internal(format!("local waste report generation failed: {e}")))
}

/// GET /api/v1/waste-report
pub async fn waste_report(
    State(state): State<Arc<AppState>>,
    Query(params): Query<WasteReportParams>,
) -> Result<Response> {
    ensure_waste_enabled(&state)?;
    let report = generate_report(&state, params.period_days).await?;
    Ok(Json(report).into_response())
}

/// GET /api/v1/waste-report/nudges
pub async fn waste_nudges(
    State(state): State<Arc<AppState>>,
    Query(params): Query<WasteReportParams>,
) -> Result<Response> {
    ensure_waste_enabled(&state)?;
    let report = generate_report(&state, params.period_days).await?;

    let mut nudges: Vec<prism_types::WasteNudge> = Vec::new();

    // ModelOverkill nudges from the overkill table
    for entry in &report.overkill {
        if entry.wasted_cost_usd < 0.01 {
            continue;
        }
        nudges.push(prism_types::WasteNudge {
            category: "model_overkill".to_string(),
            severity: "warning".to_string(),
            message: format!(
                "[Waste: Model Overkill] Using {} for {} wastes ~${:.2}/day. \
                 {} scores comparably ({:.0}% vs {:.0}%) at lower cost. \
                 Action: For {}, use a smaller model.",
                entry.expensive_model,
                entry.task_type,
                entry.wasted_cost_usd,
                entry.cheaper_alternative,
                entry.cheaper_model_score * 100.0,
                entry.expensive_model_score * 100.0,
                entry.task_type,
            ),
            savings_usd: entry.wasted_cost_usd,
        });
    }

    // WasteItem-based nudges
    for item in &report.items {
        if item.savings < 0.01 {
            continue;
        }
        let (category, severity, message) = match item.category {
            super::WasteCategory::ContextBloat => (
                "context_bloat",
                "warning",
                format!(
                    "[Waste: Context Bloat] {} calls sent large input tokens with minimal output. \
                     Action: Trim context before sending.",
                    item.call_count
                ),
            ),
            super::WasteCategory::RedundantCalls => (
                "redundant_calls",
                "warning",
                format!(
                    "[Waste: Redundant Calls] {} duplicate prompts detected. \
                     Action: Check tool result cache before re-reading.",
                    item.call_count
                ),
            ),
            super::WasteCategory::AgentLoops => (
                "agent_loops",
                "critical",
                format!(
                    "[Waste: Agent Loops] Fix-break-fix loop detected: {} calls. \
                     Action: Stop and rethink your approach before continuing.",
                    item.call_count
                ),
            ),
            super::WasteCategory::CacheMisses => (
                "cache_misses",
                "warning",
                format!(
                    "[Waste: Cache Misses] {} repeated prompts without cache hits. \
                     Action: Enable prompt caching.",
                    item.call_count
                ),
            ),
            super::WasteCategory::Overspend => (
                "overspend",
                "warning",
                format!(
                    "[Waste: Overspend] {}. \
                     Action: Consider a more cost-effective model.",
                    item.description
                ),
            ),
            super::WasteCategory::ModelOverkill => continue, // handled above from overkill table
        };
        nudges.push(prism_types::WasteNudge {
            category: category.to_string(),
            severity: severity.to_string(),
            message,
            savings_usd: item.savings,
        });
    }

    // Sort by savings descending, cap at 5
    nudges.sort_by(|a, b| {
        b.savings_usd
            .partial_cmp(&a.savings_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    nudges.truncate(5);

    Ok(Json(prism_types::WasteNudgesResponse { nudges }).into_response())
}
