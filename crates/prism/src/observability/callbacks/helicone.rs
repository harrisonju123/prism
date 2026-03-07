use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;

use crate::error::{PrismError, Result};
use crate::types::InferenceEvent;

use super::ObservabilityCallback;

pub struct HeliconeCallback {
    client: Client,
    api_key: String,
    api_url: String,
}

impl HeliconeCallback {
    pub fn new(api_key: String, api_url: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            api_url,
        }
    }
}

#[async_trait]
impl ObservabilityCallback for HeliconeCallback {
    fn name(&self) -> &str {
        "helicone"
    }

    async fn on_inference_event(&self, event: &InferenceEvent) -> Result<()> {
        let body = json!({
            "providerRequest": {
                "url": format!("https://api.{}.com/v1/chat/completions", event.provider),
                "json": { "model": &event.model },
                "meta": {
                    "Helicone-Request-Id": event.id.to_string(),
                }
            },
            "providerResponse": {
                "json": {
                    "usage": {
                        "prompt_tokens": event.input_tokens,
                        "completion_tokens": event.output_tokens,
                        "total_tokens": event.total_tokens,
                    }
                },
                "status": if matches!(event.status, crate::types::EventStatus::Success) { 200 } else { 500 },
                "headers": {}
            },
            "timing": {
                "startTime": {
                    "seconds": event.timestamp.timestamp(),
                    "milliseconds": event.timestamp.timestamp_millis() % 1000
                },
                "endTime": {
                    "seconds": event.timestamp.timestamp() + (event.latency_ms as i64 / 1000),
                    "milliseconds": (event.timestamp.timestamp_millis() + event.latency_ms as i64) % 1000
                }
            }
        });

        let resp = self
            .client
            .post(format!("{}/v1/log/request", self.api_url))
            .header("Helicone-Auth", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| PrismError::Internal(format!("helicone request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            tracing::warn!(%status, "helicone log request failed");
        }
        Ok(())
    }
}
