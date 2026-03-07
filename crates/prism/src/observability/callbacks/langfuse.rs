use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use crate::error::{PrismError, Result};
use crate::types::InferenceEvent;

use super::ObservabilityCallback;

pub struct LangfuseCallback {
    client: Client,
    api_url: String,
    public_key: String,
    secret_key: String,
}

impl LangfuseCallback {
    pub fn new(api_url: String, public_key: String, secret_key: String) -> Self {
        Self {
            client: Client::new(),
            api_url,
            public_key,
            secret_key,
        }
    }
}

#[async_trait]
impl ObservabilityCallback for LangfuseCallback {
    fn name(&self) -> &str {
        "langfuse"
    }

    async fn on_inference_event(&self, event: &InferenceEvent) -> Result<()> {
        let trace_id = event
            .trace_id
            .as_deref()
            .unwrap_or(&event.id.to_string())
            .to_string();

        let body = json!({
            "batch": [{
                "id": event.id.to_string(),
                "type": "generation-create",
                "timestamp": event.timestamp.to_rfc3339(),
                "body": {
                    "traceId": trace_id,
                    "name": format!("{}/{}", event.provider, event.model),
                    "model": &event.model,
                    "input": { "prompt_hash": &event.prompt_hash },
                    "output": { "completion_hash": &event.completion_hash },
                    "usage": {
                        "input": event.input_tokens,
                        "output": event.output_tokens,
                        "total": event.total_tokens,
                    },
                    "metadata": {
                        "provider": &event.provider,
                        "latency_ms": event.latency_ms,
                        "cost_usd": event.estimated_cost_usd,
                        "task_type": event.task_type.map(|t| t.to_string()),
                    },
                }
            }]
        });

        let resp = self
            .client
            .post(format!("{}/api/public/ingestion", self.api_url))
            .basic_auth(&self.public_key, Some(&self.secret_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| PrismError::Internal(format!("langfuse request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(%status, body = %text, "langfuse ingestion failed");
        }
        Ok(())
    }
}
