use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{PrismError, Result};
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, Choice, EmbeddingRequest, EmbeddingResponse,
    Message, PrismStreamError, ProviderResponse, Usage,
};

use super::Provider;

const ANTHROPIC_API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    api_key: String,
    api_base: String,
    client: Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String, api_base: String, client: Client) -> Self {
        Self {
            api_key,
            api_base,
            client,
        }
    }
}

// ---------------------------------------------------------------------------
// Anthropic-native request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
}

// ---------------------------------------------------------------------------
// Format conversion: OpenAI ↔ Anthropic
// ---------------------------------------------------------------------------

fn to_anthropic_request(req: &ChatCompletionRequest, model_id: &str) -> AnthropicRequest {
    let mut system = None;
    let mut messages = Vec::new();

    for msg in &req.messages {
        if msg.role == "system" {
            // Anthropic uses a top-level system field
            if let Some(content) = &msg.content {
                system = Some(content_to_string(content));
            }
        } else {
            messages.push(AnthropicMessage {
                role: msg.role.clone(),
                content: msg
                    .content
                    .clone()
                    .unwrap_or(serde_json::Value::String(String::new())),
            });
        }
    }

    // Convert OpenAI tools to Anthropic format
    let tools = req.tools.as_ref().map(|tools| {
        tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.function.name,
                    "description": t.function.description,
                    "input_schema": t.function.parameters,
                })
            })
            .collect()
    });

    AnthropicRequest {
        model: model_id.to_string(),
        max_tokens: req.max_tokens.unwrap_or(4096),
        messages,
        system,
        temperature: req.temperature,
        top_p: req.top_p,
        stream: if req.stream { Some(true) } else { None },
        tools,
    }
}

fn from_anthropic_response(resp: AnthropicResponse) -> ChatCompletionResponse {
    let text = resp
        .content
        .iter()
        .filter_map(|b| b.text.as_deref())
        .collect::<Vec<_>>()
        .join("");

    ChatCompletionResponse {
        id: resp.id,
        object: "chat.completion".into(),
        created: chrono::Utc::now().timestamp(),
        model: resp.model,
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: "assistant".into(),
                content: Some(serde_json::Value::String(text)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            },
            finish_reason: resp.stop_reason,
        }],
        usage: Some(Usage {
            prompt_tokens: resp.usage.input_tokens,
            completion_tokens: resp.usage.output_tokens,
            total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
            cache_read_input_tokens: resp.usage.cache_read_input_tokens,
            cache_creation_input_tokens: resp.usage.cache_creation_input_tokens,
        }),
        extra: Default::default(),
    }
}

fn content_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Provider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
        model_id: &str,
    ) -> Result<ProviderResponse> {
        let url = format!("{}/v1/messages", self.api_base);
        let body = to_anthropic_request(request, model_id);

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let error_body = resp.text().await.unwrap_or_default();
            return Err(PrismError::Provider(format!(
                "Anthropic returned {status}: {error_body}"
            )));
        }

        if request.stream {
            // Stream Anthropic SSE events, converting to OpenAI format on the fly
            let stream = resp
                .bytes_stream()
                .map(|result| result.map_err(PrismStreamError::Reqwest));
            Ok(ProviderResponse::Stream(Box::pin(stream)))
        } else {
            let anthropic_resp: AnthropicResponse = resp.json().await.map_err(|e| {
                PrismError::Provider(format!("failed to parse Anthropic response: {e}"))
            })?;
            Ok(ProviderResponse::Complete(from_anthropic_response(
                anthropic_resp,
            )))
        }
    }

    async fn embed(
        &self,
        _request: &EmbeddingRequest,
        _model_id: &str,
    ) -> Result<EmbeddingResponse> {
        Err(PrismError::Provider(
            "Anthropic does not support embeddings".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatCompletionRequest, Message, Tool, ToolFunction};

    fn make_provider() -> AnthropicProvider {
        AnthropicProvider::new(
            "test-key".into(),
            "https://api.anthropic.com".into(),
            Client::new(),
        )
    }

    #[test]
    fn test_name_returns_anthropic() {
        let provider = make_provider();
        assert_eq!(provider.name(), "anthropic");
    }

    #[test]
    fn test_system_message_extracted_to_top_level() {
        let req = ChatCompletionRequest {
            model: "claude-3-opus".into(),
            messages: vec![
                Message {
                    role: "system".into(),
                    content: Some(serde_json::Value::String("You are helpful.".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                Message {
                    role: "user".into(),
                    content: Some(serde_json::Value::String("Hello".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
            ],
            ..Default::default()
        };

        let anthropic_req = to_anthropic_request(&req, "claude-3-opus-20240229");
        assert_eq!(anthropic_req.system, Some("You are helpful.".into()));
        assert_eq!(anthropic_req.messages.len(), 1);
        assert_eq!(anthropic_req.messages[0].role, "user");
    }

    #[test]
    fn test_role_mapping_preserves_user_and_assistant() {
        let req = ChatCompletionRequest {
            model: "claude-3".into(),
            messages: vec![
                Message {
                    role: "user".into(),
                    content: Some(serde_json::Value::String("Hi".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                Message {
                    role: "assistant".into(),
                    content: Some(serde_json::Value::String("Hello!".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
            ],
            ..Default::default()
        };

        let anthropic_req = to_anthropic_request(&req, "claude-3");
        assert_eq!(anthropic_req.messages.len(), 2);
        assert_eq!(anthropic_req.messages[0].role, "user");
        assert_eq!(anthropic_req.messages[1].role, "assistant");
        assert!(anthropic_req.system.is_none());
    }

    #[test]
    fn test_tool_conversion_to_anthropic_format() {
        let req = ChatCompletionRequest {
            model: "claude-3".into(),
            messages: vec![Message {
                role: "user".into(),
                content: Some(serde_json::Value::String("Use tool".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            tools: Some(vec![Tool {
                r#type: "function".into(),
                function: ToolFunction {
                    name: "get_weather".into(),
                    description: Some("Get weather".into()),
                    parameters: Some(serde_json::json!({"type": "object"})),
                },
            }]),
            ..Default::default()
        };

        let anthropic_req = to_anthropic_request(&req, "claude-3");
        let tools = anthropic_req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "get_weather");
        assert_eq!(tools[0]["description"], "Get weather");
        assert_eq!(
            tools[0]["input_schema"],
            serde_json::json!({"type": "object"})
        );
    }

    #[test]
    fn test_response_conversion_joins_content_blocks() {
        let resp = AnthropicResponse {
            id: "msg_123".into(),
            model: "claude-3-opus-20240229".into(),
            content: vec![
                ContentBlock {
                    r#type: "text".into(),
                    text: Some("Hello ".into()),
                },
                ContentBlock {
                    r#type: "text".into(),
                    text: Some("world!".into()),
                },
            ],
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_input_tokens: 3,
                cache_creation_input_tokens: 2,
            },
        };

        let oai_resp = from_anthropic_response(resp);
        assert_eq!(oai_resp.id, "msg_123");
        assert_eq!(oai_resp.object, "chat.completion");
        assert_eq!(oai_resp.choices.len(), 1);
        assert_eq!(
            oai_resp.choices[0].message.content,
            Some(serde_json::Value::String("Hello world!".into()))
        );
        assert_eq!(oai_resp.choices[0].finish_reason, Some("end_turn".into()));

        let usage = oai_resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
        assert_eq!(usage.cache_read_input_tokens, 3);
        assert_eq!(usage.cache_creation_input_tokens, 2);
    }

    #[test]
    fn test_max_tokens_defaults_to_4096() {
        let req = ChatCompletionRequest {
            model: "claude-3".into(),
            messages: vec![],
            ..Default::default()
        };
        let anthropic_req = to_anthropic_request(&req, "claude-3");
        assert_eq!(anthropic_req.max_tokens, 4096);
    }
}
