use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider: String,
    pub model: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub observed_prompt_tokens: u64,
    pub observed_completion_tokens: u64,
    pub observed_cost: f64,
    pub observed_request_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceData {
    pub provider: String,
    pub model: Option<String>,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub billed_prompt_tokens: u64,
    pub billed_completion_tokens: u64,
    pub billed_cost: f64,
    pub invoice_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconciliationResult {
    pub provider: String,
    pub model: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub observed_prompt_tokens: u64,
    pub observed_completion_tokens: u64,
    pub observed_cost: f64,
    pub billed_prompt_tokens: u64,
    pub billed_completion_tokens: u64,
    pub billed_cost: f64,
    pub discrepancy_tokens: i64,
    pub discrepancy_cost: f64,
    pub discrepancy_pct_tokens: f64,
    pub discrepancy_pct_cost: f64,
    pub is_notable: bool,
}
