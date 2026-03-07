use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;

use crate::error::{PrismError, Result};
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest, EmbeddingResponse,
    PrismStreamError, ProviderResponse,
};

use super::Provider;

pub struct OpenAIProvider {
    api_key: String,
    api_base: String,
    client: Client,
}

impl OpenAIProvider {
    pub fn new(api_key: String, api_base: String, client: Client) -> Self {
        Self {
            api_key,
            api_base,
            client,
        }
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
        model_id: &str,
    ) -> Result<ProviderResponse> {
        let url = format!("{}/chat/completions", self.api_base);

        // Build provider-specific request body with the resolved model_id
        let mut body =
            serde_json::to_value(request).map_err(|e| PrismError::Internal(e.to_string()))?;
        body["model"] = serde_json::Value::String(model_id.to_string());

        // For streaming, ensure stream_options includes usage
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
                "OpenAI returned {status}: {error_body}"
            )));
        }

        if request.stream {
            // Return a byte stream for SSE relay
            let stream = resp
                .bytes_stream()
                .map(|result| result.map_err(PrismStreamError::Reqwest));
            Ok(ProviderResponse::Stream(Box::pin(stream)))
        } else {
            let response: ChatCompletionResponse = resp.json().await.map_err(|e| {
                PrismError::Provider(format!("failed to parse OpenAI response: {e}"))
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
                "OpenAI embeddings returned {status}: {error_body}"
            )));
        }

        resp.json().await.map_err(|e| {
            PrismError::Provider(format!("failed to parse OpenAI embedding response: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatCompletionRequest, Message};

    fn make_provider() -> OpenAIProvider {
        OpenAIProvider::new(
            "test-key".into(),
            "https://api.openai.com/v1".into(),
            Client::new(),
        )
    }

    #[test]
    fn test_name_returns_openai() {
        let provider = make_provider();
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn test_request_body_construction_overrides_model() {
        let req = ChatCompletionRequest {
            model: "original-model".into(),
            messages: vec![Message {
                role: "user".into(),
                content: Some(serde_json::Value::String("Hello".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            temperature: Some(0.5),
            ..Default::default()
        };

        let mut body = serde_json::to_value(&req).unwrap();
        body["model"] = serde_json::Value::String("gpt-4".to_string());

        assert_eq!(body["model"], "gpt-4");
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn test_url_assembly() {
        let provider = OpenAIProvider::new(
            "key".into(),
            "https://api.openai.com/v1".into(),
            Client::new(),
        );
        let chat_url = format!("{}/chat/completions", provider.api_base);
        let embed_url = format!("{}/embeddings", provider.api_base);
        assert_eq!(chat_url, "https://api.openai.com/v1/chat/completions");
        assert_eq!(embed_url, "https://api.openai.com/v1/embeddings");
    }
}
