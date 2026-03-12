use anyhow::Result;
use async_trait::async_trait;
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient, Method, Request as HttpRequest};
use open_ai::{OpenAiEmbeddingModel, embed};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn id(&self) -> &str;
    fn dimensions(&self) -> usize;
    /// Approximate token limit per chunk for this provider.
    fn max_tokens_per_chunk(&self) -> usize;
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

/// Routes embedding calls through the Prism gateway's `/v1/embeddings` endpoint.
/// This gives unified key management, provider routing, and rate limiting once
/// the gateway's embeddings handler is fully auth-wired.
pub struct PrismGatewayEmbeddingProvider {
    client: Arc<dyn HttpClient>,
    gateway_url: String,
    api_key: String,
    model: String,
    dimensions: usize,
}

#[derive(Serialize)]
struct GatewayEmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
}

#[derive(Deserialize)]
struct GatewayEmbeddingResponse {
    data: Vec<GatewayEmbeddingData>,
}

#[derive(Deserialize)]
struct GatewayEmbeddingData {
    embedding: serde_json::Value,
}

impl PrismGatewayEmbeddingProvider {
    pub fn new(
        client: Arc<dyn HttpClient>,
        gateway_url: String,
        api_key: String,
        model: String,
        dimensions: usize,
    ) -> Self {
        Self { client, gateway_url, api_key, model, dimensions }
    }
}

#[async_trait]
impl EmbeddingProvider for PrismGatewayEmbeddingProvider {
    fn id(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn max_tokens_per_chunk(&self) -> usize {
        8192
    }

    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let req = GatewayEmbeddingRequest { model: &self.model, input: texts };
        let body = AsyncBody::from(serde_json::to_string(&req)?);
        let request = HttpRequest::builder()
            .method(Method::POST)
            .uri(format!("{}/v1/embeddings", self.gateway_url))
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key.trim()))
            .body(body)?;

        let mut response = self.client.send(request).await?;
        let mut body_text = String::new();
        response.body_mut().read_to_string(&mut body_text).await?;

        anyhow::ensure!(
            response.status().is_success(),
            "gateway embedding request failed: status={:?} body={:?}",
            response.status(),
            body_text
        );

        let resp: GatewayEmbeddingResponse = serde_json::from_str(&body_text)?;
        resp.data
            .into_iter()
            .map(|d| parse_embedding(d.embedding))
            .collect()
    }
}

fn parse_embedding(value: serde_json::Value) -> Result<Vec<f32>> {
    match value {
        serde_json::Value::Array(arr) => arr
            .into_iter()
            .map(|v| {
                v.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| anyhow::anyhow!("expected f64 in embedding array"))
            })
            .collect(),
        _ => Err(anyhow::anyhow!("embedding field is not an array")),
    }
}

/// Wraps `open_ai::embed()` for direct API access when the gateway isn't configured.
pub struct DirectOpenAiEmbeddingProvider {
    client: Arc<dyn HttpClient>,
    api_url: String,
    api_key: String,
    model: OpenAiEmbeddingModel,
}

impl DirectOpenAiEmbeddingProvider {
    pub fn new(client: Arc<dyn HttpClient>, api_url: String, api_key: String) -> Self {
        Self { client, api_url, api_key, model: OpenAiEmbeddingModel::TextEmbedding3Small }
    }
}

#[async_trait]
impl EmbeddingProvider for DirectOpenAiEmbeddingProvider {
    fn id(&self) -> &str {
        "text-embedding-3-small"
    }

    fn dimensions(&self) -> usize {
        1536
    }

    fn max_tokens_per_chunk(&self) -> usize {
        8192
    }

    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let response =
            embed(self.client.as_ref(), &self.api_url, &self.api_key, self.model, texts.iter().copied())
                .await?;
        Ok(response.data.into_iter().map(|d| d.embedding).collect())
    }
}
