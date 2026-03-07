use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::billing::types::{InvoiceData, ProviderUsage, ReconciliationResult};
use crate::billing::{aggregator, reconciler};
use crate::error::{PrismError, Result};
use crate::keys::MasterAuth;
use crate::proxy::handler::AppState;

#[derive(Debug, Deserialize)]
pub struct ReconcileRequest {
    pub provider: Option<String>,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub invoices: Vec<InvoiceData>,
}

#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    pub provider: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// POST /api/v1/billing/reconcile
pub async fn reconcile_billing(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Json(body): Json<ReconcileRequest>,
) -> Result<Json<Vec<ReconciliationResult>>> {
    let observed = aggregator::aggregate_usage(
        &state.config.clickhouse.url,
        &state.config.clickhouse.database,
        body.provider.as_deref(),
        body.period_start,
        body.period_end,
    )
    .await
    .map_err(|e| PrismError::Internal(format!("failed to aggregate usage: {e}")))?;

    let threshold = state.config.billing.discrepancy_threshold_pct;
    let results = reconciler::reconcile(&observed, &body.invoices, threshold);

    tracing::info!(
        results = results.len(),
        notable = results.iter().filter(|r| r.is_notable).count(),
        "billing reconciliation complete"
    );

    Ok(Json(results))
}

/// GET /api/v1/billing/usage
pub async fn get_usage(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Query(params): Query<UsageQuery>,
) -> Result<Json<Vec<ProviderUsage>>> {
    let usage = aggregator::aggregate_usage(
        &state.config.clickhouse.url,
        &state.config.clickhouse.database,
        params.provider.as_deref(),
        params.start,
        params.end,
    )
    .await
    .map_err(|e| PrismError::Internal(format!("failed to aggregate usage: {e}")))?;

    Ok(Json(usage))
}
