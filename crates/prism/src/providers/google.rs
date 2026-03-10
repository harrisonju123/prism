use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{PrismError, Result};
use crate::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Choice, ChunkChoice,
    EmbeddingRequest, EmbeddingResponse, Message, MessageRole, PrismStreamError, ProviderResponse,
    Usage,
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
    #[serde(default)]
    model_version: Option<String>,
    #[serde(default)]
    response_id: Option<String>,
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

        match msg.role {
            MessageRole::System => {
                system_instruction = Some(GeminiContent {
                    role: None,
                    parts: vec![GeminiPart {
                        text: Some(text),
                        function_call: None,
                        function_response: None,
                    }],
                });
            }
            MessageRole::Assistant => {
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
                // User, Tool, and Unknown all map to "user"
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
    let parts = resp
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.content.as_ref())
        .map(|c| c.parts.as_slice())
        .unwrap_or(&[]);

    let text: String = parts
        .iter()
        .filter_map(|p| p.text.as_deref())
        .collect::<Vec<_>>()
        .join("");

    let tool_calls: Vec<serde_json::Value> = parts
        .iter()
        .filter_map(|p| p.function_call.as_ref())
        .map(|fc| {
            serde_json::json!({
                "id": format!("call_{}", uuid::Uuid::new_v4()),
                "type": "function",
                "function": {
                    "name": fc.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    "arguments": fc.get("args")
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "{}".to_string())
                }
            })
        })
        .collect();

    let (content, tool_calls_opt) = if tool_calls.is_empty() {
        (Some(serde_json::Value::String(text)), None)
    } else {
        (None, Some(tool_calls))
    };

    let finish_reason = resp
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.finish_reason.as_ref())
        .map(|r| match r.as_str() {
            "STOP" => "stop".to_string(),
            "MAX_TOKENS" => "length".to_string(),
            "SAFETY" => "content_filter".to_string(),
            "FUNCTION_CALL" => "tool_calls".to_string(),
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
                role: MessageRole::Assistant,
                content,
                name: None,
                tool_calls: tool_calls_opt,
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
// Gemini SSE parser — buffers raw bytes, extracts data: payloads
// ---------------------------------------------------------------------------

struct GeminiSseParser {
    buffer: String,
}

impl GeminiSseParser {
    fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        let mut payloads = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim_end_matches('\r').to_string();
            self.buffer.drain(..pos + 1);
            if let Some(data) = line.strip_prefix("data: ") {
                payloads.push(data.to_string());
            }
        }
        payloads
    }
}

// ---------------------------------------------------------------------------
// Gemini → OpenAI stream converter
// ---------------------------------------------------------------------------

struct GeminiStreamConverter {
    response_id: String,
    model: String,
    created: i64,
    prompt_tokens: u32,
    tool_call_index: u32,
    first_chunk: bool,
}

impl GeminiStreamConverter {
    fn new(model_id: &str) -> Self {
        Self {
            response_id: format!("gemini-{}", uuid::Uuid::new_v4()),
            model: model_id.to_string(),
            created: chrono::Utc::now().timestamp(),
            prompt_tokens: 0,
            tool_call_index: 0,
            first_chunk: true,
        }
    }

    /// Convert a single `data:` payload to zero or more OpenAI SSE `Bytes` chunks.
    fn convert(&mut self, data: &str) -> Vec<Bytes> {
        let resp: GeminiResponse = match serde_json::from_str(data) {
            Ok(r) => r,
            Err(_) => return vec![],
        };

        // Use responseId / modelVersion from first chunk if available
        if let Some(id) = &resp.response_id {
            if self.response_id.starts_with("gemini-") {
                self.response_id = id.clone();
            }
        }
        if let Some(mv) = &resp.model_version {
            self.model = mv.clone();
        }

        // Update usage from every chunk (last wins)
        if let Some(usage) = &resp.usage_metadata {
            self.prompt_tokens = usage.prompt_token_count.unwrap_or(self.prompt_tokens);
        }

        let candidate = resp.candidates.as_ref().and_then(|c| c.first());
        let parts = candidate
            .and_then(|c| c.content.as_ref())
            .map(|c| c.parts.as_slice())
            .unwrap_or(&[]);
        let finish_reason = candidate.and_then(|c| c.finish_reason.as_deref());

        let mut out: Vec<Bytes> = Vec::new();

        // Role chunk on first content-bearing event
        if self.first_chunk && (!parts.is_empty() || finish_reason.is_some()) {
            self.first_chunk = false;
            out.push(self.make_chunk(serde_json::json!({"role": "assistant"}), None, None));
        }

        // Content / tool-call chunks
        for part in parts {
            if let Some(text) = &part.text {
                out.push(self.make_chunk(serde_json::json!({"content": text}), None, None));
            } else if let Some(fc) = &part.function_call {
                let name = fc.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = fc
                    .get("args")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".to_string());
                let idx = self.tool_call_index;
                self.tool_call_index += 1;
                out.push(self.make_chunk(
                    serde_json::json!({
                        "tool_calls": [{
                            "index": idx,
                            "id": format!("call_{}", uuid::Uuid::new_v4()),
                            "type": "function",
                            "function": { "name": name, "arguments": args }
                        }]
                    }),
                    None,
                    None,
                ));
            }
        }

        // Finish chunk + DONE sentinel
        if let Some(reason) = finish_reason {
            let mapped = match reason {
                "STOP" => "stop".to_string(),
                "MAX_TOKENS" => "length".to_string(),
                "SAFETY" => "content_filter".to_string(),
                "FUNCTION_CALL" => "tool_calls".to_string(),
                other => other.to_lowercase(),
            };
            let completion_tokens = resp
                .usage_metadata
                .as_ref()
                .and_then(|u| u.candidates_token_count)
                .unwrap_or(0);
            let total = resp
                .usage_metadata
                .as_ref()
                .and_then(|u| u.total_token_count)
                .unwrap_or(self.prompt_tokens + completion_tokens);
            let usage = Usage {
                prompt_tokens: self.prompt_tokens,
                completion_tokens,
                total_tokens: total,
                ..Default::default()
            };
            out.push(self.make_chunk(serde_json::json!({}), Some(mapped), Some(usage)));
            out.push(Bytes::from("data: [DONE]\n\n"));
        }

        out
    }

    fn make_chunk(
        &self,
        delta: serde_json::Value,
        finish_reason: Option<String>,
        usage: Option<Usage>,
    ) -> Bytes {
        let chunk = ChatCompletionChunk {
            id: self.response_id.clone(),
            object: "chat.completion.chunk".into(),
            created: self.created,
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta,
                finish_reason,
            }],
            usage,
        };
        let json = serde_json::to_string(&chunk).unwrap_or_default();
        Bytes::from(format!("data: {json}\n\n"))
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
            let model_id_owned = model_id.to_string();
            let raw_stream = resp.bytes_stream();
            let stream = async_stream::try_stream! {
                let mut parser = GeminiSseParser::new();
                let mut converter = GeminiStreamConverter::new(&model_id_owned);
                let mut raw = Box::pin(raw_stream);
                while let Some(chunk_result) = raw.next().await {
                    let bytes = chunk_result.map_err(PrismStreamError::Reqwest)?;
                    for data in parser.feed(&bytes) {
                        for sse_bytes in converter.convert(&data) {
                            yield sse_bytes;
                        }
                    }
                }
            };
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
    use crate::types::{ChatCompletionRequest, Message, MessageRole};

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
                    role: MessageRole::System,
                    content: Some(serde_json::Value::String("Be concise.".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                Message {
                    role: MessageRole::User,
                    content: Some(serde_json::Value::String("Hi".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                Message {
                    role: MessageRole::Assistant,
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
            model_version: None,
            response_id: None,
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
            model_version: None,
            response_id: None,
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
            model_version: None,
            response_id: None,
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
            model_version: None,
            response_id: None,
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

    // -----------------------------------------------------------------------
    // SSE parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_gemini_sse_parser_single_event() {
        let mut parser = GeminiSseParser::new();
        let input = b"data: {\"candidates\":[]}\n\n";
        let payloads = parser.feed(input);
        assert_eq!(payloads, vec!["{\"candidates\":[]}"]);
    }

    #[test]
    fn test_gemini_sse_parser_partial_delivery() {
        let mut parser = GeminiSseParser::new();
        let p1 = parser.feed(b"data: {\"cand");
        let p2 = parser.feed(b"idates\":[]}\n\n");
        assert!(p1.is_empty());
        assert_eq!(p2, vec!["{\"candidates\":[]}"]);
    }

    #[test]
    fn test_gemini_sse_parser_multiple_events_in_one_chunk() {
        let mut parser = GeminiSseParser::new();
        let input = b"data: {\"candidates\":[]}\n\ndata: {\"candidates\":[]}\n\n";
        let payloads = parser.feed(input);
        assert_eq!(payloads.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Converter unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_gemini_converter_text_delta() {
        let mut c = GeminiStreamConverter::new("gemini-2.0-flash");
        let data = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello"}]},"finishReason":null}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"totalTokenCount":13}}"#;
        let out = c.convert(data);
        // role chunk + content chunk
        assert_eq!(out.len(), 2);
        let role_s = std::str::from_utf8(&out[0]).unwrap();
        let role_chunk: ChatCompletionChunk =
            serde_json::from_str(role_s.strip_prefix("data: ").unwrap().trim()).unwrap();
        assert_eq!(role_chunk.choices[0].delta["role"], "assistant");

        let content_s = std::str::from_utf8(&out[1]).unwrap();
        let content_chunk: ChatCompletionChunk =
            serde_json::from_str(content_s.strip_prefix("data: ").unwrap().trim()).unwrap();
        assert_eq!(content_chunk.choices[0].delta["content"], "Hello");
    }

    #[test]
    fn test_gemini_converter_finish_reason_and_done() {
        let mut c = GeminiStreamConverter::new("gemini-2.0-flash");
        let data = r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":10,"totalTokenCount":15}}"#;
        let out = c.convert(data);
        // role chunk + finish chunk + [DONE]
        assert_eq!(out.len(), 3);
        let finish_s = std::str::from_utf8(&out[1]).unwrap();
        let finish_chunk: ChatCompletionChunk =
            serde_json::from_str(finish_s.strip_prefix("data: ").unwrap().trim()).unwrap();
        assert_eq!(
            finish_chunk.choices[0].finish_reason.as_deref(),
            Some("stop")
        );
        assert_eq!(out[2], Bytes::from("data: [DONE]\n\n"));
    }

    #[test]
    fn test_gemini_converter_usage() {
        let mut c = GeminiStreamConverter::new("gemini-2.0-flash");
        let data = r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":4,"totalTokenCount":12}}"#;
        let out = c.convert(data);
        // role + finish + [DONE]
        let finish_s = std::str::from_utf8(&out[1]).unwrap();
        let finish_chunk: ChatCompletionChunk =
            serde_json::from_str(finish_s.strip_prefix("data: ").unwrap().trim()).unwrap();
        let usage = finish_chunk.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 8);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.total_tokens, 12);
    }

    #[test]
    fn test_gemini_converter_tool_call() {
        let mut c = GeminiStreamConverter::new("gemini-2.0-flash");
        let data = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"get_weather","args":{"city":"London"}}}]},"finishReason":null}]}"#;
        let out = c.convert(data);
        // role + tool_call chunk
        assert_eq!(out.len(), 2);
        let tc_s = std::str::from_utf8(&out[1]).unwrap();
        let tc_chunk: ChatCompletionChunk =
            serde_json::from_str(tc_s.strip_prefix("data: ").unwrap().trim()).unwrap();
        let calls = tc_chunk.choices[0].delta["tool_calls"].as_array().unwrap();
        assert_eq!(calls[0]["function"]["name"], "get_weather");
        // args should be a JSON string
        let args_str = calls[0]["function"]["arguments"].as_str().unwrap();
        let args_val: serde_json::Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(args_val["city"], "London");
    }

    #[test]
    fn test_function_call_response_maps_to_tool_calls() {
        let resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart {
                        text: None,
                        function_call: Some(serde_json::json!({
                            "name": "get_weather",
                            "args": {"city": "London"}
                        })),
                        function_response: None,
                    }],
                }),
                finish_reason: Some("FUNCTION_CALL".into()),
            }]),
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };

        let oai = from_gemini_response(resp, "gemini-pro");
        assert_eq!(oai.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        assert!(oai.choices[0].message.content.is_none());
        let tool_calls = oai.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
    }

    #[test]
    fn test_function_call_finish_reason() {
        let resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart {
                        text: None,
                        function_call: Some(serde_json::json!({"name": "fn", "args": {}})),
                        function_response: None,
                    }],
                }),
                finish_reason: Some("FUNCTION_CALL".into()),
            }]),
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };
        let oai = from_gemini_response(resp, "gemini-pro");
        assert_eq!(oai.choices[0].finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn test_function_call_args_serialized_to_string() {
        let resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart {
                        text: None,
                        function_call: Some(serde_json::json!({
                            "name": "get_weather",
                            "args": {"city": "London"}
                        })),
                        function_response: None,
                    }],
                }),
                finish_reason: Some("FUNCTION_CALL".into()),
            }]),
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };

        let oai = from_gemini_response(resp, "gemini-pro");
        let tool_calls = oai.choices[0].message.tool_calls.as_ref().unwrap();
        let args_str = tool_calls[0]["function"]["arguments"].as_str().unwrap();
        let args_val: serde_json::Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(args_val["city"], "London");
    }

    #[test]
    fn test_gemini_converter_finish_reason_mapping() {
        for (gemini, oai) in &[
            ("STOP", "stop"),
            ("MAX_TOKENS", "length"),
            ("SAFETY", "content_filter"),
        ] {
            let mut c = GeminiStreamConverter::new("gemini-2.0-flash");
            let data = format!(
                r#"{{"candidates":[{{"content":{{"role":"model","parts":[]}},"finishReason":"{}"}}]}}"#,
                gemini
            );
            let out = c.convert(&data);
            let finish_s = std::str::from_utf8(&out[out.len() - 2]).unwrap();
            let finish_chunk: ChatCompletionChunk =
                serde_json::from_str(finish_s.strip_prefix("data: ").unwrap().trim()).unwrap();
            assert_eq!(finish_chunk.choices[0].finish_reason.as_deref(), Some(*oai));
        }
    }

    #[test]
    fn test_gemini_converter_empty_parts_no_panic() {
        let mut c = GeminiStreamConverter::new("gemini-2.0-flash");
        // Chunk with content role only, no parts, no finishReason
        let data = r#"{"candidates":[{"content":{"role":"model","parts":[]}}]}"#;
        let out = c.convert(data);
        // No parts, no finishReason → no output
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn test_gemini_end_to_end_with_stream_relay() {
        use crate::proxy::streaming::StreamRelay;
        use futures::stream;
        use std::pin::Pin;

        let raw_events: &[&[u8]] = &[
            b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hello\"}]}}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":2,\"totalTokenCount\":12}}\n\n",
            b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\" world\"}]}}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":4,\"totalTokenCount\":14}}\n\n",
            b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":5,\"totalTokenCount\":15}}\n\n",
        ];

        let mut parser = GeminiSseParser::new();
        let mut converter = GeminiStreamConverter::new("gemini-2.0-flash");
        let mut sse_chunks: Vec<Bytes> = Vec::new();
        for raw in raw_events {
            for data in parser.feed(raw) {
                for sse in converter.convert(&data) {
                    sse_chunks.push(sse);
                }
            }
        }

        let source: Pin<
            Box<dyn futures::Stream<Item = std::result::Result<Bytes, PrismStreamError>> + Send>,
        > = Box::pin(stream::iter(
            sse_chunks
                .into_iter()
                .map(|b| Ok(b) as std::result::Result<Bytes, PrismStreamError>),
        ));
        let (_relay, result_rx) = StreamRelay::start(source);
        let result = result_rx.await.unwrap();

        assert_eq!(result.model, "gemini-2.0-flash");
        assert_eq!(result.completion_text, "Hello world");
        let usage = result.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
        assert!(result.ttft_ms.is_some());
    }
}
