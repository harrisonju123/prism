use futures::{Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use std::pin::Pin;

// Re-export prism-types for consumers
pub use prism_types::{
    AgentMetricsResponse, ChatCompletionRequest, ChatCompletionResponse, Choice, Message,
    PolicyResponse, QualityTrendsResponse, RoutingSavingsResponse, SessionEfficiencyResponse,
    SummaryResponse, TaskTypeStatsResponse, Usage, WasteScoreResponse,
};

// --- Error ---

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("api error {status}: {message}")]
    Api { status: u16, message: String },
}

pub type Result<T> = std::result::Result<T, ClientError>;

// --- Client-specific types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    pub delta: String,
    pub tool_calls: Option<serde_json::Value>,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsResponse {
    pub data: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    #[serde(default)]
    pub owned_by: String,
}

// --- SSE parsing ---

fn parse_sse_bytes(
    item: std::result::Result<bytes::Bytes, reqwest::Error>,
) -> Vec<Result<StreamChunk>> {
    let bytes = match item {
        Err(e) => return vec![Err(ClientError::Request(e))],
        Ok(b) => b,
    };
    let text = match std::str::from_utf8(&bytes) {
        Err(_) => return vec![],
        Ok(t) => t,
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                let choice = val.get("choices").and_then(|c| c.get(0));
                let delta_str = choice
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_calls = choice
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("tool_calls"))
                    .cloned();
                let finish_reason = choice
                    .and_then(|c| c.get("finish_reason"))
                    .and_then(|f| f.as_str())
                    .map(String::from);
                let usage = val
                    .get("usage")
                    .and_then(|u| serde_json::from_value(u.clone()).ok());
                out.push(Ok(StreamChunk {
                    delta: delta_str,
                    tool_calls,
                    finish_reason,
                    usage,
                }));
            }
        }
    }
    out
}

// --- Client ---

pub struct PrismClient {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl PrismClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: None,
            client: reqwest::Client::new(),
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut builder = self.client.request(method, &url);
        if let Some(ref key) = self.api_key {
            builder = builder.header("Authorization", format!("Bearer {key}"));
        }
        builder
    }

    pub async fn chat_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        let resp = self
            .request(reqwest::Method::POST, "/v1/chat/completions")
            .json(req)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let msg = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status,
                message: msg,
            });
        }
        Ok(resp.json().await?)
    }

    pub async fn stream_chat_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        let mut stream_req = req.clone();
        stream_req.stream = true;
        stream_req.stream_options = Some(prism_types::StreamOptions {
            include_usage: true,
        });

        let resp = self
            .request(reqwest::Method::POST, "/v1/chat/completions")
            .json(&stream_req)
            .send()
            .await?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let msg = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status,
                message: msg,
            });
        }

        let byte_stream = resp.bytes_stream();
        let stream = byte_stream.flat_map(|item| {
            let chunks = parse_sse_bytes(item);
            stream::iter(chunks)
        });

        Ok(Box::pin(stream))
    }

    pub async fn list_models(&self) -> Result<ModelsResponse> {
        let resp = self
            .request(reqwest::Method::GET, "/v1/models")
            .send()
            .await?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let msg = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status,
                message: msg,
            });
        }
        Ok(resp.json().await?)
    }

    pub async fn health_check(&self) -> Result<bool> {
        let resp = self.request(reqwest::Method::GET, "/health").send().await?;
        Ok(resp.status().is_success())
    }

    async fn handle_response<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let msg = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status,
                message: msg,
            });
        }
        Ok(resp.json().await?)
    }

    pub async fn stats_summary(&self, period_days: u32) -> Result<SummaryResponse> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/stats/summary?period_days={period_days}"),
            )
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn stats_waste_score(&self, period_days: u32) -> Result<WasteScoreResponse> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/stats/waste-score?period_days={period_days}"),
            )
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn stats_task_types(&self, period_days: u32) -> Result<TaskTypeStatsResponse> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/stats/task-types?period_days={period_days}"),
            )
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn routing_policy(&self) -> Result<PolicyResponse> {
        let resp = self
            .request(reqwest::Method::GET, "/api/v1/routing/policy")
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn stats_agents(&self, period_days: u32) -> Result<AgentMetricsResponse> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/stats/agents?period_days={period_days}"),
            )
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn post_feedback(
        &self,
        inference_id: Option<uuid::Uuid>,
        episode_id: Option<uuid::Uuid>,
        metric_name: &str,
        metric_value: f64,
        metadata: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "inference_id": inference_id,
            "episode_id": episode_id,
            "metric_name": metric_name,
            "metric_value": metric_value,
            "metadata": metadata,
        });
        let resp = self
            .request(reqwest::Method::POST, "/api/v1/feedback")
            .json(&body)
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn quality_trends(&self, since_days: u32) -> Result<QualityTrendsResponse> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/analytics/quality-trends?since={since_days}"),
            )
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn routing_savings(&self, since_days: u32) -> Result<RoutingSavingsResponse> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/analytics/routing-savings?since={since_days}"),
            )
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn session_efficiency(&self, since_days: u32) -> Result<SessionEfficiencyResponse> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/v1/analytics/session-efficiency?since={since_days}"),
            )
            .send()
            .await?;
        Self::handle_response(resp).await
    }
}
