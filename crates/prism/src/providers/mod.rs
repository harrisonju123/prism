pub mod anthropic;
pub mod azure_openai;
#[cfg(feature = "aws")]
pub mod bedrock;
pub mod google;
pub mod groq;
pub mod mistral;
pub mod openai;

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::config::ProviderConfig;
use crate::error::{PrismError, Result};
use crate::types::{ChatCompletionRequest, EmbeddingRequest, EmbeddingResponse, ProviderResponse};

/// Trait that all LLM providers implement.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Forward a chat completion request.
    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
        model_id: &str,
    ) -> Result<ProviderResponse>;

    /// Forward an embedding request.
    async fn embed(&self, request: &EmbeddingRequest, model_id: &str) -> Result<EmbeddingResponse>;

    /// Provider name for logging and metrics.
    fn name(&self) -> &'static str;
}

/// Registry of configured providers.
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn Provider>>,
}

impl ProviderRegistry {
    /// Build registry from provider configs.
    pub fn from_config(
        configs: &HashMap<String, ProviderConfig>,
        http_client: reqwest::Client,
    ) -> Self {
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();

        for (name, config) in configs {
            // If provider_type is set to "openai_compatible", skip the name match
            // and go straight to OpenAI-compatible instantiation.
            let is_openai_compatible = config.provider_type.as_deref() == Some("openai_compatible");

            let provider: Option<Arc<dyn Provider>> = if is_openai_compatible {
                let key = resolve_env_var(config.api_key.as_deref().unwrap_or(""));
                let base = config
                    .api_base
                    .as_deref()
                    .unwrap_or("https://api.openai.com/v1");
                tracing::info!(
                    provider = name,
                    api_base = base,
                    "registering as openai-compatible provider"
                );
                Some(Arc::new(openai::OpenAIProvider::new(
                    key,
                    base.to_string(),
                    http_client.clone(),
                )))
            } else {
                match name.as_str() {
                    "openai" => {
                        let key = resolve_env_var(config.api_key.as_deref().unwrap_or(""));
                        let base = config
                            .api_base
                            .as_deref()
                            .unwrap_or("https://api.openai.com/v1");
                        Some(Arc::new(openai::OpenAIProvider::new(
                            key,
                            base.to_string(),
                            http_client.clone(),
                        )))
                    }
                    "anthropic" => {
                        let key = resolve_env_var(config.api_key.as_deref().unwrap_or(""));
                        let base = config
                            .api_base
                            .as_deref()
                            .unwrap_or("https://api.anthropic.com");
                        Some(Arc::new(anthropic::AnthropicProvider::new(
                            key,
                            base.to_string(),
                            http_client.clone(),
                        )))
                    }
                    "google" => {
                        let key = resolve_env_var(config.api_key.as_deref().unwrap_or(""));
                        Some(Arc::new(google::GoogleProvider::new(
                            key,
                            http_client.clone(),
                        )))
                    }
                    "groq" => {
                        let key = resolve_env_var(config.api_key.as_deref().unwrap_or(""));
                        let base = config
                            .api_base
                            .as_deref()
                            .unwrap_or("https://api.groq.com/openai/v1");
                        Some(Arc::new(groq::GroqProvider::new(
                            key,
                            base.to_string(),
                            http_client.clone(),
                        )))
                    }
                    "mistral" => {
                        let key = resolve_env_var(config.api_key.as_deref().unwrap_or(""));
                        let base = config
                            .api_base
                            .as_deref()
                            .unwrap_or("https://api.mistral.ai/v1");
                        Some(Arc::new(mistral::MistralProvider::new(
                            key,
                            base.to_string(),
                            http_client.clone(),
                        )))
                    }
                    // DeepSeek and Together use OpenAI-compatible format
                    "deepseek" => {
                        let key = resolve_env_var(config.api_key.as_deref().unwrap_or(""));
                        let base = config
                            .api_base
                            .as_deref()
                            .unwrap_or("https://api.deepseek.com/v1");
                        Some(Arc::new(openai::OpenAIProvider::new(
                            key,
                            base.to_string(),
                            http_client.clone(),
                        )))
                    }
                    "together" => {
                        let key = resolve_env_var(config.api_key.as_deref().unwrap_or(""));
                        let base = config
                            .api_base
                            .as_deref()
                            .unwrap_or("https://api.together.xyz/v1");
                        Some(Arc::new(openai::OpenAIProvider::new(
                            key,
                            base.to_string(),
                            http_client.clone(),
                        )))
                    }
                    "azure" | "azure_openai" => {
                        let key = resolve_env_var(config.api_key.as_deref().unwrap_or(""));
                        let resource_name = config
                            .extra
                            .get("resource_name")
                            .cloned()
                            .unwrap_or_default();
                        let deployment_id = config
                            .extra
                            .get("deployment_id")
                            .cloned()
                            .unwrap_or_default();
                        let api_version = config.extra.get("api_version").cloned();
                        if resource_name.is_empty() || deployment_id.is_empty() {
                            tracing::warn!(
                                provider = name,
                                "azure provider requires resource_name and deployment_id in extra config, skipping"
                            );
                            None
                        } else {
                            Some(Arc::new(azure_openai::AzureOpenAIProvider::new(
                                key,
                                resource_name,
                                deployment_id,
                                api_version,
                                http_client.clone(),
                            )))
                        }
                    }
                    #[cfg(feature = "aws")]
                    "bedrock" | "aws_bedrock" => {
                        let region = config.region.clone();
                        let provider = tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current()
                                .block_on(bedrock::BedrockProvider::new(region))
                        });
                        tracing::info!(provider = name, "registering bedrock provider");
                        Some(Arc::new(provider))
                    }
                    _ => {
                        if config.api_base.is_some() {
                            let key = resolve_env_var(config.api_key.as_deref().unwrap_or(""));
                            let base = config.api_base.as_deref().unwrap();
                            tracing::info!(
                                provider = name,
                                api_base = base,
                                "unknown provider with api_base, registering as openai-compatible"
                            );
                            Some(Arc::new(openai::OpenAIProvider::new(
                                key,
                                base.to_string(),
                                http_client.clone(),
                            )))
                        } else {
                            tracing::warn!(
                                provider = name,
                                "unknown provider without api_base, skipping"
                            );
                            None
                        }
                    }
                }
            };

            if let Some(p) = provider {
                providers.insert(name.clone(), p);
            }
        }

        Self { providers }
    }

    /// Get a provider by name.
    pub fn get(&self, name: &str) -> Result<&Arc<dyn Provider>> {
        self.providers
            .get(name)
            .ok_or_else(|| PrismError::ProviderNotConfigured(name.to_string()))
    }

    /// List configured provider names.
    pub fn list(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }
}

/// Resolve environment variable references like "${OPENAI_API_KEY}".
fn resolve_env_var(value: &str) -> String {
    if value.starts_with("${") && value.ends_with('}') {
        let var_name = &value[2..value.len() - 1];
        std::env::var(var_name).unwrap_or_default()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(
        api_key: Option<&str>,
        api_base: Option<&str>,
        provider_type: Option<&str>,
    ) -> ProviderConfig {
        ProviderConfig {
            api_key: api_key.map(|s| s.to_string()),
            api_base: api_base.map(|s| s.to_string()),
            provider_type: provider_type.map(|s| s.to_string()),
            region: None,
            extra: HashMap::new(),
        }
    }

    #[test]
    fn test_known_provider_registered() {
        let mut configs = HashMap::new();
        configs.insert(
            "openai".to_string(),
            make_config(Some("test-key"), None, None),
        );
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        assert!(registry.get("openai").is_ok());
        assert_eq!(registry.get("openai").unwrap().name(), "openai");
    }

    #[test]
    fn test_unknown_provider_with_api_base_registered() {
        let mut configs = HashMap::new();
        configs.insert(
            "fireworks".to_string(),
            make_config(
                Some("fw-key"),
                Some("https://api.fireworks.ai/inference/v1"),
                None,
            ),
        );
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        assert!(registry.get("fireworks").is_ok());
        // Uses OpenAIProvider under the hood
        assert_eq!(registry.get("fireworks").unwrap().name(), "openai");
    }

    #[test]
    fn test_unknown_provider_without_api_base_skipped() {
        let mut configs = HashMap::new();
        configs.insert("unknown_no_base".to_string(), make_config(None, None, None));
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        assert!(registry.get("unknown_no_base").is_err());
    }

    #[test]
    fn test_openai_compatible_provider_type() {
        let mut configs = HashMap::new();
        configs.insert(
            "ollama".to_string(),
            make_config(
                None,
                Some("http://localhost:11434/v1"),
                Some("openai_compatible"),
            ),
        );
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        assert!(registry.get("ollama").is_ok());
        assert_eq!(registry.get("ollama").unwrap().name(), "openai");
    }

    #[test]
    fn test_openai_compatible_type_overrides_known_name() {
        // Even if the name matches a known provider like "anthropic",
        // provider_type = "openai_compatible" should force OpenAI provider.
        let mut configs = HashMap::new();
        configs.insert(
            "anthropic".to_string(),
            make_config(
                Some("key"),
                Some("https://custom-proxy.example.com/v1"),
                Some("openai_compatible"),
            ),
        );
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        assert!(registry.get("anthropic").is_ok());
        assert_eq!(registry.get("anthropic").unwrap().name(), "openai");
    }

    #[test]
    fn test_openai_compatible_default_base() {
        // provider_type = "openai_compatible" with no api_base should
        // default to the OpenAI base URL.
        let mut configs = HashMap::new();
        configs.insert(
            "custom".to_string(),
            make_config(Some("key"), None, Some("openai_compatible")),
        );
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        assert!(registry.get("custom").is_ok());
    }

    #[test]
    fn test_resolve_env_var_literal() {
        assert_eq!(resolve_env_var("plain-key"), "plain-key");
    }

    #[test]
    fn test_resolve_env_var_reference() {
        unsafe { std::env::set_var("PRISM_TEST_KEY_1234", "resolved-value") };
        assert_eq!(resolve_env_var("${PRISM_TEST_KEY_1234}"), "resolved-value");
        unsafe { std::env::remove_var("PRISM_TEST_KEY_1234") };
    }

    #[test]
    fn test_resolve_env_var_missing() {
        assert_eq!(resolve_env_var("${PRISM_NONEXISTENT_VAR_XYZ}"), "");
    }

    fn make_azure_config(resource_name: &str, deployment_id: &str) -> ProviderConfig {
        let mut extra = HashMap::new();
        extra.insert("resource_name".to_string(), resource_name.to_string());
        extra.insert("deployment_id".to_string(), deployment_id.to_string());
        ProviderConfig {
            api_key: Some("test-azure-key".to_string()),
            api_base: None,
            provider_type: None,
            region: None,
            extra,
        }
    }

    #[test]
    fn test_azure_provider_registered() {
        let mut configs = HashMap::new();
        configs.insert(
            "azure".to_string(),
            make_azure_config("my-resource", "gpt-4"),
        );
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        assert!(registry.get("azure").is_ok());
        assert_eq!(registry.get("azure").unwrap().name(), "azure_openai");
    }

    #[test]
    fn test_azure_openai_name_registered() {
        let mut configs = HashMap::new();
        configs.insert("azure_openai".to_string(), make_azure_config("res", "dep"));
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        assert!(registry.get("azure_openai").is_ok());
        assert_eq!(registry.get("azure_openai").unwrap().name(), "azure_openai");
    }

    #[test]
    fn test_azure_missing_resource_name_skipped() {
        let mut extra = HashMap::new();
        extra.insert("deployment_id".to_string(), "gpt-4".to_string());
        let config = ProviderConfig {
            api_key: Some("key".to_string()),
            api_base: None,
            provider_type: None,
            region: None,
            extra,
        };
        let mut configs = HashMap::new();
        configs.insert("azure".to_string(), config);
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        assert!(registry.get("azure").is_err());
    }

    #[test]
    fn test_azure_missing_deployment_id_skipped() {
        let mut extra = HashMap::new();
        extra.insert("resource_name".to_string(), "my-resource".to_string());
        let config = ProviderConfig {
            api_key: Some("key".to_string()),
            api_base: None,
            provider_type: None,
            region: None,
            extra,
        };
        let mut configs = HashMap::new();
        configs.insert("azure".to_string(), config);
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        assert!(registry.get("azure").is_err());
    }

    #[test]
    fn test_registry_list_providers() {
        let mut configs = HashMap::new();
        configs.insert("openai".to_string(), make_config(Some("k"), None, None));
        configs.insert(
            "vllm".to_string(),
            make_config(None, Some("http://localhost:8000/v1"), None),
        );
        let registry = ProviderRegistry::from_config(&configs, reqwest::Client::new());
        let mut listed = registry.list();
        listed.sort();
        assert_eq!(listed, vec!["openai", "vllm"]);
    }
}
