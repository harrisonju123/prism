use chrono::{DateTime, Utc};

use super::types::ProviderUsage;

pub async fn aggregate_usage(
    ch_url: &str,
    ch_db: &str,
    provider: Option<&str>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> anyhow::Result<Vec<ProviderUsage>> {
    let client = reqwest::Client::new();

    let provider_filter = provider
        .map(|p| format!(" AND provider = '{p}'"))
        .unwrap_or_default();

    let query = format!(
        "SELECT provider, model, \
         min(timestamp) as period_start, max(timestamp) as period_end, \
         sum(input_tokens) as observed_prompt_tokens, \
         sum(output_tokens) as observed_completion_tokens, \
         sum(estimated_cost_usd) as observed_cost, \
         count() as observed_request_count \
         FROM {db}.inference_events \
         WHERE timestamp >= '{start}' AND timestamp < '{end}'{filter} \
         GROUP BY provider, model \
         FORMAT JSONEachRow",
        db = ch_db,
        start = start.format("%Y-%m-%d %H:%M:%S"),
        end = end.format("%Y-%m-%d %H:%M:%S"),
        filter = provider_filter,
    );

    let resp = client.post(ch_url).body(query).send().await?.text().await?;

    let mut results = Vec::new();
    for line in resp.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(row) = serde_json::from_str::<serde_json::Value>(line) {
            let usage = ProviderUsage {
                provider: row
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                model: row
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                period_start: start,
                period_end: end,
                observed_prompt_tokens: row
                    .get("observed_prompt_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                observed_completion_tokens: row
                    .get("observed_completion_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                observed_cost: row
                    .get("observed_cost")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                observed_request_count: row
                    .get("observed_request_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            };
            results.push(usage);
        }
    }

    Ok(results)
}
