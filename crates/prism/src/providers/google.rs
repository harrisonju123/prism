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

pub struct GoogleProvider {
    api_key: String,
    client: Client,
}

impl GoogleProvider {
    pub fn new(api_key: String, client: Client) -> Self {
        Self { api_key, client }
    }
}

// ---------------------------------------------------------------------------
// Google Gemini request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionCall")]
    function_call: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionResponse")]
    function_response: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct GeminiTool {
    function_declarations: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsage {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
}

// ---------------------------------------------------------------------------
// Format conversion: OpenAI → Gemini
// ---------------------------------------------------------------------------

fn to_gemini_request(req: &ChatCompletionRequest, model_id: &str) -> (String, GeminiRequest) {
    let mut contents = Vec::new();
    let mut system_instruction = None;

    for msg in &req.messages {
        let text = msg
            .content
            .as_ref()
            .map(|c| match c {
                serde_json::Value::String(s) => s.clone(),
                _ => c.to_string(),
            })
            .unwrap_or_default();

        match msg.role.as_str() {
            "system" => {
                system_instruction = Some(GeminiContent {
                    role: None,
                    parts: vec![GeminiPart {
                        text: Some(text),
                        function_call: None,
                        function_response: None,
                    }],
                });
            }
            "assistant" => {
                contents.push(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart {
                        text: Some(text),
                        function_call: None,
                        function_response: None,
                    }],
                });
            }
            _ => {
                // "user" and "tool" both map to "user"
                contents.push(GeminiContent {
                    role: Some("user".to_string()),
                    parts: vec![GeminiPart {
                        text: Some(text),
                        function_call: None,
                        function_response: None,
                    }],
                });
            }
        }
    }

    let generation_config = Some(GenerationConfig {
        temperature: req.temperature,
        top_p: req.top_p,
        max_output_tokens: req.max_tokens,
        stop_sequences: req.stop.as_ref().and_then(|s| match s {
            serde_json::Value::String(s) => Some(vec![s.clone()]),
            serde_json::Value::Array(arr) => Some(
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
            ),
            _ => None,
        }),
    });

    let tools = req.tools.as_ref().map(|tools| {
        vec![GeminiTool {
            function_declarations: tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.function.name,
                        "description": t.function.description,
                        "parameters": t.function.parameters,
                    })
                })
                .collect(),
        }]
    });

    (
        model_id.to_string(),
        GeminiRequest {
            contents,
            system_instruction,
            generation_config,
            tools,
        },
    )
}

fn from_gemini_response(resp: GeminiResponse, model_id: &str) -> ChatCompletionResponse {
    let text = resp
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.content.as_ref())
        .map(|c| {
            c.parts
                .iter()
                .filter_map(|p| p.text.as_deref())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    let finish_reason = resp
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.finish_reason.as_ref())
        .map(|r| match r.as_str() {
            "STOP" => "stop".to_string(),
            "MAX_TOKENS" => "length".to_string(),
            "SAFETY" => "content_filter".to_string(),
            other => other.to_lowercase(),
        });

    let usage = resp.usage_metadata.as_ref().map(|u| {
        let prompt = u.prompt_token_count.unwrap_or(0);
        let completion = u.candidates_token_count.unwrap_or(0);
        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: u.total_token_count.unwrap_or(prompt + completion),
            ..Default::default()
        }
    });

    ChatCompletionResponse {
        id: format!("gemini-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".into(),
        created: chrono::Utc::now().timestamp(),
        model: model_id.to_string(),
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
            finish_reason,
        }],
        usage,
        extra: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// Provider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for GoogleProvider {
    fn name(&self) -> &'static str {
        "google"
    }

    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
        model_id: &str,
    ) -> Result<ProviderResponse> {
        let (model, body) = to_gemini_request(request, model_id);

        let method = if request.stream {
            "streamGenerateContent?alt=sse"
        } else {
            "generateContent"
        };

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:{}",
            model, method,
        );

        let resp = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let error_body = resp.text().await.unwrap_or_default();
            return Err(PrismError::Provider(format!(
                "Google returned {status}: {error_body}"
            )));
        }

        if request.stream {
            let stream = resp
                .bytes_stream()
                .map(|result| result.map_err(PrismStreamError::Reqwest));
            Ok(ProviderResponse::Stream(Box::pin(stream)))
        } else {
            let gemini_resp: GeminiResponse = resp.json().await.map_err(|e| {
                PrismError::Provider(format!("failed to parse Google response: {e}"))
            })?;
            Ok(ProviderResponse::Complete(from_gemini_response(
                gemini_resp,
                model_id,
            )))
        }
    }

    async fn embed(&self, request: &EmbeddingRequest, model_id: &str) -> Result<EmbeddingResponse> {
        let input_text = match &request.input {
            serde_json::Value::String(s) => vec![s.clone()],
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => {
                return Err(PrismError::BadRequest(
                    "invalid embedding input format".into(),
                ));
            }
        };

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:batchEmbedContents",
            model_id,
        );

        let requests: Vec<serde_json::Value> = input_text
            .iter()
            .map(|text| {
                serde_json::json!({
                    "model": format!("models/{}", model_id),
                    "content": { "parts": [{ "text": text }] },
                })
            })
            .collect();

        let body = serde_json::json!({ "requests": requests });

        let resp = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let error_body = resp.text().await.unwrap_or_default();
            return Err(PrismError::Provider(format!(
                "Google embeddings returned {status}: {error_body}"
            )));
        }

        let resp_body: serde_json::Value = resp.json().await.map_err(|e| {
            PrismError::Provider(format!("failed to parse Google embedding response: {e}"))
        })?;

        let embeddings = resp_body
            .get("embeddings")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .enumerate()
                    .map(|(i, e)| crate::types::EmbeddingData {
                        object: "embedding".to_string(),
                        index: i as u32,
                        embedding: e
                            .get("values")
                            .cloned()
                            .unwrap_or(serde_json::Value::Array(vec![])),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(EmbeddingResponse {
            object: "list".to_string(),
            data: embeddings,
            model: model_id.to_string(),
            usage: crate::types::EmbeddingUsage {
                prompt_tokens: input_text.iter().map(|t| t.len() as u32 / 4).sum(),
                total_tokens: input_text.iter().map(|t| t.len() as u32 / 4).sum(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatCompletionRequest, Message};

    fn make_provider() -> GoogleProvider {
        GoogleProvider::new("test-key".into(), Client::new())
    }

    #[test]
    fn test_name_returns_google() {
        let provider = make_provider();
        assert_eq!(provider.name(), "google");
    }

    #[test]
    fn test_role_mapping_assistant_to_model_and_system_to_instruction() {
        let req = ChatCompletionRequest {
            model: "gemini-pro".into(),
            messages: vec![
                Message {
                    role: "system".into(),
                    content: Some(serde_json::Value::String("Be concise.".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
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

        let (_model, gemini_req) = to_gemini_request(&req, "gemini-pro");

        // System message should be extracted to system_instruction
        let sys = gemini_req.system_instruction.unwrap();
        assert_eq!(sys.parts[0].text.as_deref(), Some("Be concise."));

        // Only user and assistant messages remain in contents
        assert_eq!(gemini_req.contents.len(), 2);
        assert_eq!(gemini_req.contents[0].role.as_deref(), Some("user"));
        assert_eq!(gemini_req.contents[1].role.as_deref(), Some("model"));
    }

    #[test]
    fn test_generation_config_construction() {
        let req = ChatCompletionRequest {
            model: "gemini-pro".into(),
            messages: vec![],
            temperature: Some(0.7),
            top_p: Some(0.9),
            max_tokens: Some(1024),
            stop: Some(serde_json::Value::String("END".into())),
            ..Default::default()
        };

        let (_model, gemini_req) = to_gemini_request(&req, "gemini-pro");
        let config = gemini_req.generation_config.unwrap();
        assert_eq!(config.temperature, Some(0.7));
        assert_eq!(config.top_p, Some(0.9));
        assert_eq!(config.max_output_tokens, Some(1024));
        assert_eq!(config.stop_sequences, Some(vec!["END".to_string()]));
    }

    #[test]
    fn test_response_finish_reason_mapping() {
        let resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart {
                        text: Some("Done.".into()),
                        function_call: None,
                        function_response: None,
                    }],
                }),
                finish_reason: Some("STOP".into()),
            }]),
            usage_metadata: None,
        };
        let oai = from_gemini_response(resp, "gemini-pro");
        assert_eq!(oai.choices[0].finish_reason.as_deref(), Some("stop"));

        // MAX_TOKENS -> length
        let resp2 = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart {
                        text: Some("...".into()),
                        function_call: None,
                        function_response: None,
                    }],
                }),
                finish_reason: Some("MAX_TOKENS".into()),
            }]),
            usage_metadata: None,
        };
        let oai2 = from_gemini_response(resp2, "gemini-pro");
        assert_eq!(oai2.choices[0].finish_reason.as_deref(), Some("length"));

        // SAFETY -> content_filter
        let resp3 = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart {
                        text: Some("".into()),
                        function_call: None,
                        function_response: None,
                    }],
                }),
                finish_reason: Some("SAFETY".into()),
            }]),
            usage_metadata: None,
        };
        let oai3 = from_gemini_response(resp3, "gemini-pro");
        assert_eq!(
            oai3.choices[0].finish_reason.as_deref(),
            Some("content_filter")
        );
    }

    #[test]
    fn test_usage_extraction() {
        let resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart {
                        text: Some("Hi".into()),
                        function_call: None,
                        function_response: None,
                    }],
                }),
                finish_reason: Some("STOP".into()),
            }]),
            usage_metadata: Some(GeminiUsage {
                prompt_token_count: Some(10),
                candidates_token_count: Some(5),
                total_token_count: Some(15),
            }),
        };

        let oai = from_gemini_response(resp, "gemini-pro");
        let usage = oai.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn test_model_id_returned_in_request_tuple() {
        let req = ChatCompletionRequest {
            model: "gemini-pro".into(),
            messages: vec![],
            ..Default::default()
        };
        let (model_id, _) = to_gemini_request(&req, "gemini-1.5-pro");
        assert_eq!(model_id, "gemini-1.5-pro");
    }
}
