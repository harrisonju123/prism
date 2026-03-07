use chrono::{DateTime, Utc};

use super::types::InvoiceData;

pub fn parse_invoices(raw: &[serde_json::Value]) -> Vec<InvoiceData> {
    raw.iter().filter_map(parse_single_invoice).collect()
}

fn parse_single_invoice(val: &serde_json::Value) -> Option<InvoiceData> {
    let provider = val.get("provider").and_then(|v| v.as_str())?.to_string();
    let model = val.get("model").and_then(|v| v.as_str()).map(String::from);
    let period_start = parse_datetime(val.get("period_start")?)?;
    let period_end = parse_datetime(val.get("period_end")?)?;
    let invoice_id = val
        .get("invoice_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let billed_prompt_tokens = val
        .get("billed_prompt_tokens")
        .or_else(|| val.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let billed_completion_tokens = val
        .get("billed_completion_tokens")
        .or_else(|| val.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let billed_cost = val
        .get("billed_cost")
        .or_else(|| val.get("cost"))
        .or_else(|| val.get("amount"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    Some(InvoiceData {
        provider,
        model,
        period_start,
        period_end,
        billed_prompt_tokens,
        billed_completion_tokens,
        billed_cost,
        invoice_id,
    })
}

fn parse_datetime(val: &serde_json::Value) -> Option<DateTime<Utc>> {
    val.as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_anthropic_format() {
        let raw = vec![json!({
            "provider": "anthropic",
            "model": "claude-3-opus",
            "period_start": "2024-01-01T00:00:00Z",
            "period_end": "2024-01-31T23:59:59Z",
            "billed_prompt_tokens": 1000000,
            "billed_completion_tokens": 500000,
            "billed_cost": 45.00,
            "invoice_id": "inv_001"
        })];
        let invoices = parse_invoices(&raw);
        assert_eq!(invoices.len(), 1);
        assert_eq!(invoices[0].provider, "anthropic");
        assert_eq!(invoices[0].model.as_deref(), Some("claude-3-opus"));
        assert_eq!(invoices[0].billed_prompt_tokens, 1000000);
        assert!((invoices[0].billed_cost - 45.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_openai_format() {
        let raw = vec![json!({
            "provider": "openai",
            "period_start": "2024-01-01T00:00:00Z",
            "period_end": "2024-01-31T23:59:59Z",
            "input_tokens": 2000000,
            "output_tokens": 800000,
            "cost": 30.00,
            "invoice_id": "inv_002"
        })];
        let invoices = parse_invoices(&raw);
        assert_eq!(invoices.len(), 1);
        assert_eq!(invoices[0].provider, "openai");
        assert!(invoices[0].model.is_none());
        assert_eq!(invoices[0].billed_prompt_tokens, 2000000);
    }

    #[test]
    fn parse_empty() {
        let invoices = parse_invoices(&[]);
        assert!(invoices.is_empty());
    }

    #[test]
    fn parse_invalid_skipped() {
        let raw = vec![json!({"foo": "bar"})];
        let invoices = parse_invoices(&raw);
        assert!(invoices.is_empty());
    }
}
