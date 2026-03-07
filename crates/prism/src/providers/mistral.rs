use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;

use crate::error::{PrismError, Result};
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest, EmbeddingResponse,
    PrismStreamError, ProviderResponse,
};

use super::Provider;

/// Mistral provider — OpenAI-compatible API.
pub struct MistralProvider {
    api_key: String,
    api_base: String,
    client: Client,
}

impl MistralProvider {
    pub fn new(api_key: String, api_base: String, client: Client) -> Self {
        Self {
            api_key,
            api_base,
            client,
        }
    }
}

#[async_trait]
impl Provider for MistralProvider {
    fn name(&self) -> &'static str {
        "mistral"
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
                "Mistral returned {status}: {error_body}"
            )));
        }

        if request.stream {
            let stream = resp
                .bytes_stream()
                .map(|result| result.map_err(PrismStreamError::Reqwest));
            Ok(ProviderResponse::Stream(Box::pin(stream)))
        } else {
            let response: ChatCompletionResponse = resp.json().await.map_err(|e| {
                PrismError::Provider(format!("failed to parse Mistral response: {e}"))
            })?;
            Ok(ProviderResponse::Complete(response))
        }
    }

    async fn embed(&self, request: &EmbeddingRequest, model_id: &str) -> Result<EmbeddingResponse> {
        let url = format!("{}/embeddings", self.api_base);

        let mut body =
            serde_json::to_value(request).map_err(|e| PrismError::Internal(e.to_string()))?;
        body["model"] = serde_json::Value::String(model_id.to_string());

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
                "Mistral embeddings returned {status}: {error_body}"
            )));
        }

        resp.json().await.map_err(|e| {
            PrismError::Provider(format!("failed to parse Mistral embedding response: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider() -> MistralProvider {
        MistralProvider::new(
            "test-key".into(),
            "https://api.mistral.ai/v1".into(),
            Client::new(),
        )
    }

    #[test]
    fn test_name_returns_mistral() {
        let provider = make_provider();
        assert_eq!(provider.name(), "mistral");
    }

    #[test]
    fn test_url_construction() {
        let provider = make_provider();
        let chat_url = format!("{}/chat/completions", provider.api_base);
        let embed_url = format!("{}/embeddings", provider.api_base);
        assert_eq!(chat_url, "https://api.mistral.ai/v1/chat/completions");
        assert_eq!(embed_url, "https://api.mistral.ai/v1/embeddings");
    }

    #[test]
    fn test_request_body_model_override() {
        let req = ChatCompletionRequest {
            model: "original".into(),
            messages: vec![],
            ..Default::default()
        };
        let mut body = serde_json::to_value(&req).unwrap();
        body["model"] = serde_json::Value::String("mistral-large-latest".to_string());
        assert_eq!(body["model"], "mistral-large-latest");
    }
}
