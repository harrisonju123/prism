use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::alerts::types::{AlertChannel, RuleType};
use crate::error::Result;
use crate::proxy::handler::AppState;

#[derive(Debug, Serialize)]
pub struct AlertRulesResponse {
    pub rules: Vec<AlertRuleView>,
}

#[derive(Debug, Serialize)]
pub struct AlertRuleView {
    pub id: uuid::Uuid,
    pub rule_type: RuleType,
    pub threshold: f64,
    pub channel: AlertChannel,
    pub webhook_url: Option<String>,
    pub enabled: bool,
}

impl From<&crate::config::AlertRuleConfig> for AlertRuleView {
    fn from(r: &crate::config::AlertRuleConfig) -> Self {
        let rule_type = match r.rule_type.as_str() {
            "spend_threshold" => RuleType::SpendThreshold,
            "anomaly_zscore" => RuleType::AnomalyZscore,
            "error_rate" => RuleType::ErrorRate,
            "latency_p95" => RuleType::LatencyP95,
            _ => RuleType::SpendThreshold,
        };
        let channel = match r.channel.as_str() {
            "webhook" => AlertChannel::Webhook,
            _ => AlertChannel::Log,
        };
        AlertRuleView {
            id: r.id,
            rule_type,
            threshold: r.threshold,
            channel,
            webhook_url: r.webhook_url.clone(),
            enabled: r.enabled,
        }
    }
}

/// GET /api/v1/alerts/rules — list configured alert rules.
pub async fn list_rules(State(state): State<Arc<AppState>>) -> Result<Response> {
    let rules: Vec<AlertRuleView> = state
        .config
        .alerts
        .rules
        .iter()
        .map(AlertRuleView::from)
        .collect();

    Ok(Json(AlertRulesResponse { rules }).into_response())
}
