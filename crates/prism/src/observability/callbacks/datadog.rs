use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use crate::error::{PrismError, Result};
use crate::types::InferenceEvent;

use super::ObservabilityCallback;

pub struct DatadogCallback {
    client: Client,
    api_key: String,
    site: String,
}

impl DatadogCallback {
    pub fn new(api_key: String, site: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            site,
        }
    }
}

#[async_trait]
impl ObservabilityCallback for DatadogCallback {
    fn name(&self) -> &str {
        "datadog"
    }

    async fn on_inference_event(&self, event: &InferenceEvent) -> Result<()> {
        let log_entry = json!({
            "ddsource": "prism",
            "ddtags": format!("provider:{},model:{}", event.provider, event.model),
            "hostname": "prism-gateway",
            "service": "prism",
            "message": format!(
                "inference provider={} model={} tokens={} cost={:.6} latency_ms={}",
                event.provider, event.model, event.total_tokens,
                event.estimated_cost_usd, event.latency_ms
            ),
            "inference": {
                "id": event.id.to_string(),
                "provider": &event.provider,
                "model": &event.model,
                "input_tokens": event.input_tokens,
                "output_tokens": event.output_tokens,
                "total_tokens": event.total_tokens,
                "cost_usd": event.estimated_cost_usd,
                "latency_ms": event.latency_ms,
                "status": format!("{:?}", event.status),
                "task_type": event.task_type.map(|t| t.to_string()),
                "trace_id": &event.trace_id,
            }
        });

        let resp = self
            .client
            .post(format!(
                "https://http-intake.logs.{}/api/v2/logs",
                self.site
            ))
            .header("DD-API-KEY", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&[log_entry])
            .send()
            .await
            .map_err(|e| PrismError::Internal(format!("datadog request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            tracing::warn!(%status, "datadog log ingestion failed");
        }
        Ok(())
    }
}
