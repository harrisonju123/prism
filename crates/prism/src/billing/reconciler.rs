use std::collections::HashMap;

use super::types::{InvoiceData, ProviderUsage, ReconciliationResult};

pub fn reconcile(
    observed: &[ProviderUsage],
    billed: &[InvoiceData],
    threshold_pct: f64,
) -> Vec<ReconciliationResult> {
    let mut results = Vec::new();

    // Index observed by (provider, model)
    let mut observed_map: HashMap<(String, String), &ProviderUsage> = HashMap::new();
    // Aggregate by provider for provider-level matching
    let mut provider_agg: HashMap<String, AggregatedUsage> = HashMap::new();
    let mut matched_observed: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    for usage in observed {
        observed_map.insert((usage.provider.clone(), usage.model.clone()), usage);
        let agg = provider_agg.entry(usage.provider.clone()).or_default();
        agg.prompt_tokens += usage.observed_prompt_tokens;
        agg.completion_tokens += usage.observed_completion_tokens;
        agg.cost += usage.observed_cost;
        agg.period_start = agg
            .period_start
            .map(|s| s.min(usage.period_start))
            .or(Some(usage.period_start));
        agg.period_end = agg
            .period_end
            .map(|e| e.max(usage.period_end))
            .or(Some(usage.period_end));
    }

    // Process each invoice
    for invoice in billed {
        if let Some(ref model) = invoice.model {
            // Model-level match
            let key = (invoice.provider.clone(), model.clone());
            if let Some(obs) = observed_map.get(&key) {
                matched_observed.insert(key);
                results.push(build_result(
                    &invoice.provider,
                    model,
                    obs.period_start,
                    obs.period_end,
                    obs.observed_prompt_tokens,
                    obs.observed_completion_tokens,
                    obs.observed_cost,
                    invoice.billed_prompt_tokens,
                    invoice.billed_completion_tokens,
                    invoice.billed_cost,
                    threshold_pct,
                ));
            } else {
                // Invoice with no matching observed data
                results.push(build_result(
                    &invoice.provider,
                    model,
                    invoice.period_start,
                    invoice.period_end,
                    0,
                    0,
                    0.0,
                    invoice.billed_prompt_tokens,
                    invoice.billed_completion_tokens,
                    invoice.billed_cost,
                    threshold_pct,
                ));
            }
        } else {
            // Provider-level match (no model specified on invoice)
            if let Some(agg) = provider_agg.get(&invoice.provider) {
                // Mark all observed entries for this provider as matched
                for usage in observed {
                    if usage.provider == invoice.provider {
                        matched_observed.insert((usage.provider.clone(), usage.model.clone()));
                    }
                }
                let ps = agg.period_start.unwrap_or(invoice.period_start);
                let pe = agg.period_end.unwrap_or(invoice.period_end);
                results.push(build_result(
                    &invoice.provider,
                    "(all models)",
                    ps,
                    pe,
                    agg.prompt_tokens,
                    agg.completion_tokens,
                    agg.cost,
                    invoice.billed_prompt_tokens,
                    invoice.billed_completion_tokens,
                    invoice.billed_cost,
                    threshold_pct,
                ));
            } else {
                results.push(build_result(
                    &invoice.provider,
                    "(all models)",
                    invoice.period_start,
                    invoice.period_end,
                    0,
                    0,
                    0.0,
                    invoice.billed_prompt_tokens,
                    invoice.billed_completion_tokens,
                    invoice.billed_cost,
                    threshold_pct,
                ));
            }
        }
    }

    // Report unmatched observed entries
    for usage in observed {
        let key = (usage.provider.clone(), usage.model.clone());
        if !matched_observed.contains(&key) {
            results.push(build_result(
                &usage.provider,
                &usage.model,
                usage.period_start,
                usage.period_end,
                usage.observed_prompt_tokens,
                usage.observed_completion_tokens,
                usage.observed_cost,
                0,
                0,
                0.0,
                threshold_pct,
            ));
        }
    }

    results
}

#[derive(Default)]
struct AggregatedUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    cost: f64,
    period_start: Option<chrono::DateTime<chrono::Utc>>,
    period_end: Option<chrono::DateTime<chrono::Utc>>,
}

fn build_result(
    provider: &str,
    model: &str,
    period_start: chrono::DateTime<chrono::Utc>,
    period_end: chrono::DateTime<chrono::Utc>,
    obs_prompt: u64,
    obs_completion: u64,
    obs_cost: f64,
    billed_prompt: u64,
    billed_completion: u64,
    billed_cost: f64,
    threshold_pct: f64,
) -> ReconciliationResult {
    let obs_total = obs_prompt + obs_completion;
    let billed_total = billed_prompt + billed_completion;
    let discrepancy_tokens = billed_total as i64 - obs_total as i64;
    let discrepancy_cost = billed_cost - obs_cost;

    let discrepancy_pct_tokens = if obs_total > 0 {
        discrepancy_tokens as f64 / obs_total as f64
    } else if billed_total > 0 {
        1.0
    } else {
        0.0
    };

    let discrepancy_pct_cost = if obs_cost > 0.0 {
        discrepancy_cost / obs_cost
    } else if billed_cost > 0.0 {
        1.0
    } else {
        0.0
    };

    let is_notable = discrepancy_pct_cost.abs() > threshold_pct;

    ReconciliationResult {
        provider: provider.to_string(),
        model: model.to_string(),
        period_start,
        period_end,
        observed_prompt_tokens: obs_prompt,
        observed_completion_tokens: obs_completion,
        observed_cost: obs_cost,
        billed_prompt_tokens: billed_prompt,
        billed_completion_tokens: billed_completion,
        billed_cost: billed_cost,
        discrepancy_tokens,
        discrepancy_cost,
        discrepancy_pct_tokens,
        discrepancy_pct_cost,
        is_notable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_usage(
        provider: &str,
        model: &str,
        prompt: u64,
        completion: u64,
        cost: f64,
    ) -> ProviderUsage {
        ProviderUsage {
            provider: provider.into(),
            model: model.into(),
            period_start: Utc::now(),
            period_end: Utc::now(),
            observed_prompt_tokens: prompt,
            observed_completion_tokens: completion,
            observed_cost: cost,
            observed_request_count: 100,
        }
    }

    fn make_invoice(
        provider: &str,
        model: Option<&str>,
        prompt: u64,
        completion: u64,
        cost: f64,
    ) -> InvoiceData {
        InvoiceData {
            provider: provider.into(),
            model: model.map(String::from),
            period_start: Utc::now(),
            period_end: Utc::now(),
            billed_prompt_tokens: prompt,
            billed_completion_tokens: completion,
            billed_cost: cost,
            invoice_id: "inv_test".into(),
        }
    }

    #[test]
    fn exact_match() {
        let observed = vec![make_usage("openai", "gpt-4o", 1000, 500, 10.0)];
        let billed = vec![make_invoice("openai", Some("gpt-4o"), 1000, 500, 10.0)];
        let results = reconcile(&observed, &billed, 0.02);
        assert_eq!(results.len(), 1);
        assert!(!results[0].is_notable);
        assert!((results[0].discrepancy_cost).abs() < f64::EPSILON);
    }

    #[test]
    fn overbilled() {
        let observed = vec![make_usage("openai", "gpt-4o", 1000, 500, 10.0)];
        let billed = vec![make_invoice("openai", Some("gpt-4o"), 1000, 500, 15.0)];
        let results = reconcile(&observed, &billed, 0.02);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_notable);
        assert!(results[0].discrepancy_cost > 0.0);
    }

    #[test]
    fn underbilled() {
        let observed = vec![make_usage("openai", "gpt-4o", 1000, 500, 15.0)];
        let billed = vec![make_invoice("openai", Some("gpt-4o"), 1000, 500, 10.0)];
        let results = reconcile(&observed, &billed, 0.02);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_notable);
        assert!(results[0].discrepancy_cost < 0.0);
    }

    #[test]
    fn unmatched_observed() {
        let observed = vec![make_usage("anthropic", "claude-3", 500, 200, 5.0)];
        let billed: Vec<InvoiceData> = vec![];
        let results = reconcile(&observed, &billed, 0.02);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].billed_cost, 0.0);
    }

    #[test]
    fn unmatched_invoice() {
        let observed: Vec<ProviderUsage> = vec![];
        let billed = vec![make_invoice("openai", Some("gpt-4o"), 1000, 500, 10.0)];
        let results = reconcile(&observed, &billed, 0.02);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].observed_cost, 0.0);
        assert!(results[0].is_notable);
    }

    #[test]
    fn provider_level_matching() {
        let observed = vec![
            make_usage("openai", "gpt-4o", 500, 200, 5.0),
            make_usage("openai", "gpt-4o-mini", 300, 100, 2.0),
        ];
        let billed = vec![make_invoice("openai", None, 800, 300, 7.0)];
        let results = reconcile(&observed, &billed, 0.02);
        assert_eq!(results.len(), 1);
        assert!(!results[0].is_notable);
    }

    #[test]
    fn within_threshold() {
        let observed = vec![make_usage("openai", "gpt-4o", 1000, 500, 10.0)];
        let billed = vec![make_invoice("openai", Some("gpt-4o"), 1000, 500, 10.01)];
        let results = reconcile(&observed, &billed, 0.02);
        assert_eq!(results.len(), 1);
        assert!(!results[0].is_notable); // 0.1% < 2% threshold
    }
}
