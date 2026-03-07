use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;

use crate::error::{PrismError, Result};
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest, EmbeddingResponse,
    PrismStreamError, ProviderResponse,
};

use super::Provider;

/// Groq provider — OpenAI-compatible API with ultra-low latency inference.
pub struct GroqProvider {
    api_key: String,
    api_base: String,
    client: Client,
}

impl GroqProvider {
    pub fn new(api_key: String, api_base: String, client: Client) -> Self {
        Self {
            api_key,
            api_base,
            client,
        }
    }
}

#[async_trait]
impl Provider for GroqProvider {
    fn name(&self) -> &'static str {
        "groq"
    }

    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
        model_id: &str,
    ) -> Result<ProviderResponse> {
        let url = format!("{}/chat/completions", self.api_base);

        let mut body =
            serde_json::to_value(request).map_err(|e| PrismError::Internal(e.to_string()))?;
        body["model"] = serde_json::Value::String(model_id.to_string());

        if request.stream {
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let error_body = resp.text().await.unwrap_or_default();
            return Err(PrismError::Provider(format!(
                "Groq returned {status}: {error_body}"
            )));
        }

        if request.stream {
            let stream = resp
                .bytes_stream()
                .map(|result| result.map_err(PrismStreamError::Reqwest));
            Ok(ProviderResponse::Stream(Box::pin(stream)))
        } else {
            let response: ChatCompletionResponse = resp
                .json()
                .await
                .map_err(|e| PrismError::Provider(format!("failed to parse Groq response: {e}")))?;
            Ok(ProviderResponse::Complete(response))
        }
    }

    async fn embed(
        &self,
        _request: &EmbeddingRequest,
        _model_id: &str,
    ) -> Result<EmbeddingResponse> {
        Err(PrismError::Provider(
            "Groq does not support embeddings".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EmbeddingRequest;

    fn make_provider() -> GroqProvider {
        GroqProvider::new(
            "test-key".into(),
            "https://api.groq.com/openai/v1".into(),
            Client::new(),
        )
    }

    #[test]
    fn test_name_returns_groq() {
        let provider = make_provider();
        assert_eq!(provider.name(), "groq");
    }

    #[tokio::test]
    async fn test_embed_returns_error() {
        let provider = make_provider();
        let req = EmbeddingRequest {
            model: "test".into(),
            input: serde_json::Value::String("hello".into()),
            encoding_format: None,
            extra: Default::default(),
        };
        let result = provider.embed(&req, "test").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does not support embeddings"));
    }

    #[test]
    fn test_url_construction() {
        let provider = make_provider();
        let chat_url = format!("{}/chat/completions", provider.api_base);
        assert_eq!(chat_url, "https://api.groq.com/openai/v1/chat/completions");
    }
}
