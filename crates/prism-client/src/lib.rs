use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

// Re-export prism-types for consumers
pub use prism_types::{ChatCompletionRequest, ChatCompletionResponse, Choice, Message, Usage};

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
    pub finish_reason: Option<String>,
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
        use bytes::Bytes;
        use futures::StreamExt;

        let mut stream_req = req.clone();
        stream_req.stream = true;

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
        let stream = byte_stream.filter_map(|item| async move {
            let bytes: Bytes = item.ok()?;
            let text = std::str::from_utf8(&bytes).ok()?;
            // SSE: each line may be "data: {...}" or "data: [DONE]"
            let mut chunks = Vec::new();
            for line in text.lines() {
                let line = line.trim();
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                        let delta = val
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("delta"))
                            .and_then(|d| d.get("content"))
                            .and_then(|c| c.as_str())
                            .unwrap_or("")
                            .to_string();
                        let finish_reason = val
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("finish_reason"))
                            .and_then(|f| f.as_str())
                            .map(String::from);
                        chunks.push(Ok(StreamChunk {
                            delta,
                            finish_reason,
                        }));
                    }
                }
            }
            if chunks.is_empty() {
                None
            } else {
                chunks.into_iter().next()
            }
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
}
