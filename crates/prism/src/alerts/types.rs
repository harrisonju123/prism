use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleType {
    SpendThreshold,
    AnomalyZscore,
    ErrorRate,
    LatencyP95,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertChannel {
    Webhook,
    Log,
    Slack,
    Email,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: Uuid,
    pub rule_type: RuleType,
    pub threshold: f64,
    pub channel: AlertChannel,
    pub webhook_url: Option<String>,
    pub slack_webhook_url: Option<String>,
    pub email_to: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AlertEvent {
    pub rule_id: Uuid,
    pub triggered_at: String,
    pub message: String,
    pub severity: AlertSeverity,
    pub current_value: Option<f64>,
    pub threshold_value: Option<f64>,
}
