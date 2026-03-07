use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;

use crate::error::{PrismError, Result};
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest, EmbeddingResponse,
    PrismStreamError, ProviderResponse,
};

use super::Provider;

pub struct AzureOpenAIProvider {
    api_key: String,
    resource_name: String,
    deployment_id: String,
    api_version: String,
    client: Client,
}

impl AzureOpenAIProvider {
    pub fn new(
        api_key: String,
        resource_name: String,
        deployment_id: String,
        api_version: Option<String>,
        client: Client,
    ) -> Self {
        Self {
            api_key,
            resource_name,
            deployment_id,
            api_version: api_version.unwrap_or_else(|| "2024-02-01".to_string()),
            client,
        }
    }

    fn chat_completions_url(&self) -> String {
        format!(
            "https://{}.openai.azure.com/openai/deployments/{}/chat/completions?api-version={}",
            self.resource_name, self.deployment_id, self.api_version
        )
    }

    fn embeddings_url(&self) -> String {
        format!(
            "https://{}.openai.azure.com/openai/deployments/{}/embeddings?api-version={}",
            self.resource_name, self.deployment_id, self.api_version
        )
    }
}

#[async_trait]
impl Provider for AzureOpenAIProvider {
    fn name(&self) -> &'static str {
        "azure_openai"
    }

    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
        model_id: &str,
    ) -> Result<ProviderResponse> {
        let url = self.chat_completions_url();

        let mut body =
            serde_json::to_value(request).map_err(|e| PrismError::Internal(e.to_string()))?;
        body["model"] = serde_json::Value::String(model_id.to_string());

        if request.stream {
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }

        let resp = self
            .client
            .post(&url)
            .header("api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let error_body = resp.text().await.unwrap_or_default();
            return Err(PrismError::Provider(format!(
                "Azure OpenAI returned {status}: {error_body}"
            )));
        }

        if request.stream {
            let stream = resp
                .bytes_stream()
                .map(|result| result.map_err(PrismStreamError::Reqwest));
            Ok(ProviderResponse::Stream(Box::pin(stream)))
        } else {
            let response: ChatCompletionResponse = resp.json().await.map_err(|e| {
                PrismError::Provider(format!("failed to parse Azure OpenAI response: {e}"))
            })?;
            Ok(ProviderResponse::Complete(response))
        }
    }

    async fn embed(&self, request: &EmbeddingRequest, model_id: &str) -> Result<EmbeddingResponse> {
        let url = self.embeddings_url();

        let mut body =
            serde_json::to_value(request).map_err(|e| PrismError::Internal(e.to_string()))?;
        body["model"] = serde_json::Value::String(model_id.to_string());

        let resp = self
            .client
            .post(&url)
            .header("api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let error_body = resp.text().await.unwrap_or_default();
            return Err(PrismError::Provider(format!(
                "Azure OpenAI embeddings returned {status}: {error_body}"
            )));
        }

        resp.json().await.map_err(|e| {
            PrismError::Provider(format!(
                "failed to parse Azure OpenAI embedding response: {e}"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_completions_url() {
        let provider = AzureOpenAIProvider::new(
            "test-key".to_string(),
            "my-resource".to_string(),
            "gpt-4".to_string(),
            None,
            Client::new(),
        );
        assert_eq!(
            provider.chat_completions_url(),
            "https://my-resource.openai.azure.com/openai/deployments/gpt-4/chat/completions?api-version=2024-02-01"
        );
    }

    #[test]
    fn test_embeddings_url() {
        let provider = AzureOpenAIProvider::new(
            "test-key".to_string(),
            "my-resource".to_string(),
            "text-embedding-ada-002".to_string(),
            None,
            Client::new(),
        );
        assert_eq!(
            provider.embeddings_url(),
            "https://my-resource.openai.azure.com/openai/deployments/text-embedding-ada-002/embeddings?api-version=2024-02-01"
        );
    }

    #[test]
    fn test_custom_api_version() {
        let provider = AzureOpenAIProvider::new(
            "test-key".to_string(),
            "my-resource".to_string(),
            "gpt-4".to_string(),
            Some("2024-06-01".to_string()),
            Client::new(),
        );
        assert_eq!(
            provider.chat_completions_url(),
            "https://my-resource.openai.azure.com/openai/deployments/gpt-4/chat/completions?api-version=2024-06-01"
        );
    }

    #[test]
    fn test_default_api_version() {
        let provider = AzureOpenAIProvider::new(
            "key".to_string(),
            "res".to_string(),
            "dep".to_string(),
            None,
            Client::new(),
        );
        assert_eq!(provider.api_version, "2024-02-01");
    }

    #[test]
    fn test_name() {
        let provider = AzureOpenAIProvider::new(
            "key".to_string(),
            "res".to_string(),
            "dep".to_string(),
            None,
            Client::new(),
        );
        assert_eq!(provider.name(), "azure_openai");
    }
}
