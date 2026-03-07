use crate::models;
use crate::types::Usage;

/// Compute estimated cost in USD from usage and model name.
/// Accounts for Anthropic prompt caching:
/// - cache_read_input_tokens cost 90% less than normal input tokens
/// - cache_creation_input_tokens cost 25% more than normal input tokens
pub fn compute_cost(model: &str, usage: &Usage) -> f64 {
    let Some(info) = models::lookup_model(model) else {
        return 0.0;
    };

    let input_per_token = info.input_cost_per_token();
    let output_per_token = info.output_cost_per_token();

    // Base input tokens (excluding cached tokens)
    let base_input = usage
        .prompt_tokens
        .saturating_sub(usage.cache_read_input_tokens)
        .saturating_sub(usage.cache_creation_input_tokens);

    let base_cost = base_input as f64 * input_per_token;
    let cache_read_cost = usage.cache_read_input_tokens as f64 * input_per_token * 0.10; // 90% discount
    let cache_create_cost = usage.cache_creation_input_tokens as f64 * input_per_token * 1.25; // 25% surcharge
    let output_cost = usage.completion_tokens as f64 * output_per_token;

    base_cost + cache_read_cost + cache_create_cost + output_cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_without_cache() {
        let usage = Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            ..Default::default()
        };
        let cost = compute_cost("claude-sonnet-4", &usage);
        // input: 1000 * 3.0/1M = 0.003, output: 500 * 15.0/1M = 0.0075
        assert!((cost - 0.0105).abs() < 1e-9);
    }

    #[test]
    fn cost_with_cache_read() {
        let usage = Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_input_tokens: 800,
            cache_creation_input_tokens: 0,
        };
        let cost = compute_cost("claude-sonnet-4", &usage);
        // base input: 200 * 3.0/1M = 0.0006
        // cache read: 800 * 3.0/1M * 0.10 = 0.00024
        // output: 500 * 15.0/1M = 0.0075
        let expected = 0.0006 + 0.00024 + 0.0075;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn cost_with_cache_creation() {
        let usage = Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 300,
        };
        let cost = compute_cost("claude-sonnet-4", &usage);
        // base input: 700 * 3.0/1M = 0.0021
        // cache create: 300 * 3.0/1M * 1.25 = 0.001125
        // output: 500 * 15.0/1M = 0.0075
        let expected = 0.0021 + 0.001125 + 0.0075;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn cost_unknown_model() {
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            ..Default::default()
        };
        assert_eq!(compute_cost("unknown-model-xyz", &usage), 0.0);
    }
}
