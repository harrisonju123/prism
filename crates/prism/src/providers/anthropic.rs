use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::{PrismError, Result};
use crate::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Choice, ChunkChoice,
    EmbeddingRequest, EmbeddingResponse, Message, PrismStreamError, ProviderResponse, Usage,
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
// Anthropic streaming event types (private)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamEvent {
    MessageStart {
        message: AnthropicStreamMessage,
    },
    ContentBlockStart {
        index: u32,
        content_block: AnthropicStreamContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: AnthropicStreamDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: AnthropicMessageDelta,
        #[serde(default)]
        usage: Option<AnthropicStreamUsage>,
    },
    MessageStop,
    Ping,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamMessage {
    id: String,
    model: String,
    usage: AnthropicStreamUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct AnthropicStreamUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
}

// ---------------------------------------------------------------------------
// Anthropic SSE parser — buffers raw bytes, extracts data: payloads
// ---------------------------------------------------------------------------

struct AnthropicSseParser {
    buffer: String,
}

impl AnthropicSseParser {
    fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Feed raw bytes. Returns any complete `data:` line payloads extracted.
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
// Anthropic → OpenAI stream converter
// ---------------------------------------------------------------------------

struct ContentBlockInfo {
    block_type: String,
    tool_call_index: u32,
}

struct AnthropicStreamConverter {
    message_id: String,
    model: String,
    created: i64,
    input_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    tool_call_counter: u32,
    active_blocks: HashMap<u32, ContentBlockInfo>,
}

impl AnthropicStreamConverter {
    fn new() -> Self {
        Self {
            message_id: String::new(),
            model: String::new(),
            created: chrono::Utc::now().timestamp(),
            input_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            tool_call_counter: 0,
            active_blocks: HashMap::new(),
        }
    }

    /// Convert a single `data:` payload to OpenAI SSE bytes. Returns `None` for no-op events.
    fn convert(&mut self, data: &str) -> Option<Bytes> {
        if data == "[DONE]" {
            return None;
        }
        let event: AnthropicStreamEvent = serde_json::from_str(data).ok()?;
        let chunk = match event {
            AnthropicStreamEvent::MessageStart { message } => {
                self.message_id = message.id;
                self.model = message.model;
                self.input_tokens = message.usage.input_tokens;
                self.cache_read_tokens = message.usage.cache_read_input_tokens;
                self.cache_creation_tokens = message.usage.cache_creation_input_tokens;
                Some(self.make_chunk(
                    serde_json::json!({"role": "assistant"}),
                    None,
                    None,
                ))
            }
            AnthropicStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                if content_block.block_type == "tool_use" {
                    let tool_index = self.tool_call_counter;
                    self.tool_call_counter += 1;
                    self.active_blocks.insert(
                        index,
                        ContentBlockInfo {
                            block_type: "tool_use".into(),
                            tool_call_index: tool_index,
                        },
                    );
                    Some(self.make_chunk(
                        serde_json::json!({
                            "tool_calls": [{
                                "index": tool_index,
                                "id": content_block.id.unwrap_or_default(),
                                "type": "function",
                                "function": {
                                    "name": content_block.name.unwrap_or_default(),
                                    "arguments": ""
                                }
                            }]
                        }),
                        None,
                        None,
                    ))
                } else {
                    self.active_blocks.insert(
                        index,
                        ContentBlockInfo {
                            block_type: content_block.block_type,
                            tool_call_index: 0,
                        },
                    );
                    None
                }
            }
            AnthropicStreamEvent::ContentBlockDelta { index, delta } => match delta {
                AnthropicStreamDelta::TextDelta { text } => {
                    Some(self.make_chunk(serde_json::json!({"content": text}), None, None))
                }
                AnthropicStreamDelta::InputJsonDelta { partial_json } => {
                    let tool_index = self
                        .active_blocks
                        .get(&index)
                        .map(|b| b.tool_call_index)
                        .unwrap_or(0);
                    Some(self.make_chunk(
                        serde_json::json!({
                            "tool_calls": [{
                                "index": tool_index,
                                "function": {"arguments": partial_json}
                            }]
                        }),
                        None,
                        None,
                    ))
                }
                AnthropicStreamDelta::Unknown => None,
            },
            AnthropicStreamEvent::ContentBlockStop { .. } => None,
            AnthropicStreamEvent::MessageDelta { delta, usage } => {
                let finish_reason = delta
                    .stop_reason
                    .as_deref()
                    .map(|r| map_stop_reason(r).to_string());
                let output_tokens = usage.as_ref().map(|u| u.output_tokens).unwrap_or(0);
                let usage_obj = Usage {
                    prompt_tokens: self.input_tokens,
                    completion_tokens: output_tokens,
                    total_tokens: self.input_tokens + output_tokens,
                    cache_read_input_tokens: self.cache_read_tokens,
                    cache_creation_input_tokens: self.cache_creation_tokens,
                };
                Some(self.make_chunk(serde_json::json!({}), finish_reason, Some(usage_obj)))
            }
            AnthropicStreamEvent::MessageStop => {
                return Some(Bytes::from("data: [DONE]\n\n"));
            }
            AnthropicStreamEvent::Ping | AnthropicStreamEvent::Unknown => None,
        };

        chunk.map(|c| {
            let json = serde_json::to_string(&c).unwrap_or_default();
            Bytes::from(format!("data: {json}\n\n"))
        })
    }

    fn make_chunk(
        &self,
        delta: serde_json::Value,
        finish_reason: Option<String>,
        usage: Option<Usage>,
    ) -> ChatCompletionChunk {
        ChatCompletionChunk {
            id: self.message_id.clone(),
            object: "chat.completion.chunk".into(),
            created: self.created,
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta,
                finish_reason,
            }],
            usage,
        }
    }
}

fn map_stop_reason(reason: &str) -> &str {
    match reason {
        "end_turn" => "stop",
        "max_tokens" => "length",
        "stop_sequence" => "stop",
        "tool_use" => "tool_calls",
        other => other,
    }
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
            finish_reason: resp.stop_reason.as_deref().map(map_stop_reason).map(str::to_string),
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
            let raw_stream = resp.bytes_stream();
            let stream = async_stream::try_stream! {
                let mut parser = AnthropicSseParser::new();
                let mut converter = AnthropicStreamConverter::new();
                let mut raw = Box::pin(raw_stream);
                while let Some(chunk_result) = raw.next().await {
                    let bytes = chunk_result.map_err(PrismStreamError::Reqwest)?;
                    for data in parser.feed(&bytes) {
                        if let Some(sse_bytes) = converter.convert(&data) {
                            yield sse_bytes;
                        }
                    }
                }
            };
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
        assert_eq!(oai_resp.choices[0].finish_reason, Some("stop".into()));

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

    // -----------------------------------------------------------------------
    // SSE parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_sse_parser_single_event() {
        let mut parser = AnthropicSseParser::new();
        let input = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";
        let payloads = parser.feed(input);
        assert_eq!(payloads, vec!["{\"type\":\"ping\"}"]);
    }

    #[test]
    fn test_sse_parser_partial_delivery() {
        let mut parser = AnthropicSseParser::new();
        // Payload split across two chunk deliveries
        let p1 = parser.feed(b"data: {\"type\":\"pin");
        let p2 = parser.feed(b"g\"}\n\n");
        assert!(p1.is_empty());
        assert_eq!(p2, vec!["{\"type\":\"ping\"}"]);
    }

    #[test]
    fn test_sse_parser_multiple_events_in_one_chunk() {
        let mut parser = AnthropicSseParser::new();
        let input = b"data: {\"type\":\"ping\"}\n\ndata: {\"type\":\"ping\"}\n\n";
        let payloads = parser.feed(input);
        assert_eq!(payloads.len(), 2);
    }

    #[test]
    fn test_sse_parser_ignores_event_lines() {
        let mut parser = AnthropicSseParser::new();
        let input = b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let payloads = parser.feed(input);
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0], "{\"type\":\"message_stop\"}");
    }

    // -----------------------------------------------------------------------
    // Converter unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_converter_message_start_produces_role_chunk() {
        let mut c = AnthropicStreamConverter::new();
        let data = r#"{"type":"message_start","message":{"id":"msg_123","model":"claude-3-5-sonnet","usage":{"input_tokens":10,"output_tokens":0,"cache_read_input_tokens":5,"cache_creation_input_tokens":2}}}"#;
        let result = c.convert(data).unwrap();
        let s = std::str::from_utf8(&result).unwrap();
        assert!(s.starts_with("data: "));
        let chunk: ChatCompletionChunk =
            serde_json::from_str(s.strip_prefix("data: ").unwrap().trim()).unwrap();
        assert_eq!(chunk.id, "msg_123");
        assert_eq!(chunk.model, "claude-3-5-sonnet");
        assert_eq!(chunk.choices[0].delta["role"], "assistant");
        assert!(chunk.usage.is_none());
    }

    #[test]
    fn test_converter_text_delta_produces_content_chunk() {
        let mut c = AnthropicStreamConverter::new();
        c.convert(r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-3","usage":{"input_tokens":5,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#);

        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello, world!"}}"#;
        let result = c.convert(data).unwrap();
        let s = std::str::from_utf8(&result).unwrap();
        let chunk: ChatCompletionChunk =
            serde_json::from_str(s.strip_prefix("data: ").unwrap().trim()).unwrap();
        assert_eq!(chunk.choices[0].delta["content"], "Hello, world!");
    }

    #[test]
    fn test_converter_message_delta_finish_reason_and_usage() {
        let mut c = AnthropicStreamConverter::new();
        c.convert(r#"{"type":"message_start","message":{"id":"msg_t","model":"claude-3","usage":{"input_tokens":10,"output_tokens":0,"cache_read_input_tokens":5,"cache_creation_input_tokens":2}}}"#);

        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":20}}"#;
        let result = c.convert(data).unwrap();
        let s = std::str::from_utf8(&result).unwrap();
        let chunk: ChatCompletionChunk =
            serde_json::from_str(s.strip_prefix("data: ").unwrap().trim()).unwrap();
        assert_eq!(chunk.choices[0].finish_reason, Some("stop".into()));
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 20);
        assert_eq!(usage.total_tokens, 30);
        assert_eq!(usage.cache_read_input_tokens, 5);
        assert_eq!(usage.cache_creation_input_tokens, 2);
    }

    #[test]
    fn test_converter_message_stop_produces_done() {
        let mut c = AnthropicStreamConverter::new();
        let result = c.convert(r#"{"type":"message_stop"}"#).unwrap();
        assert_eq!(result, Bytes::from("data: [DONE]\n\n"));
    }

    #[test]
    fn test_converter_ping_produces_no_output() {
        let mut c = AnthropicStreamConverter::new();
        assert!(c.convert(r#"{"type":"ping"}"#).is_none());
    }

    #[test]
    fn test_converter_content_block_stop_produces_no_output() {
        let mut c = AnthropicStreamConverter::new();
        assert!(c.convert(r#"{"type":"content_block_stop","index":0}"#).is_none());
    }

    #[test]
    fn test_converter_tool_use_block() {
        let mut c = AnthropicStreamConverter::new();
        c.convert(r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-3","usage":{"input_tokens":5,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#);

        // Tool use content block start
        let start = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_abc","name":"get_weather"}}"#;
        let result = c.convert(start).unwrap();
        let s = std::str::from_utf8(&result).unwrap();
        let chunk: ChatCompletionChunk =
            serde_json::from_str(s.strip_prefix("data: ").unwrap().trim()).unwrap();
        let calls = chunk.choices[0].delta["tool_calls"].as_array().unwrap();
        assert_eq!(calls[0]["index"], 0);
        assert_eq!(calls[0]["id"], "toolu_abc");
        assert_eq!(calls[0]["type"], "function");
        assert_eq!(calls[0]["function"]["name"], "get_weather");
        assert_eq!(calls[0]["function"]["arguments"], "");

        // Argument streaming
        let delta = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"city\":\""}}"#;
        let result2 = c.convert(delta).unwrap();
        let s2 = std::str::from_utf8(&result2).unwrap();
        let chunk2: ChatCompletionChunk =
            serde_json::from_str(s2.strip_prefix("data: ").unwrap().trim()).unwrap();
        let calls2 = chunk2.choices[0].delta["tool_calls"].as_array().unwrap();
        assert_eq!(calls2[0]["index"], 0);
        assert_eq!(calls2[0]["function"]["arguments"], "{\"city\":\"");
    }

    #[test]
    fn test_stop_reason_mapping() {
        assert_eq!(map_stop_reason("end_turn"), "stop");
        assert_eq!(map_stop_reason("max_tokens"), "length");
        assert_eq!(map_stop_reason("stop_sequence"), "stop");
        assert_eq!(map_stop_reason("tool_use"), "tool_calls");
        assert_eq!(map_stop_reason("unknown_reason"), "unknown_reason");
    }

    #[tokio::test]
    async fn test_end_to_end_with_stream_relay() {
        use crate::proxy::streaming::StreamRelay;
        use futures::stream;
        use std::pin::Pin;

        // Simulate Anthropic SSE byte chunks (as they arrive from the wire)
        let raw_events: &[&[u8]] = &[
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_e2e\",\"model\":\"claude-3-5-sonnet\",\"usage\":{\"input_tokens\":10,\"output_tokens\":0,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}}\n\n",
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            b"event: ping\ndata: {\"type\":\"ping\"}\n\n",
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\n",
            b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":5}}\n\n",
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ];

        // Run through parser + converter
        let mut parser = AnthropicSseParser::new();
        let mut converter = AnthropicStreamConverter::new();
        let mut sse_chunks: Vec<Bytes> = Vec::new();
        for raw in raw_events {
            for data in parser.feed(raw) {
                if let Some(sse) = converter.convert(&data) {
                    sse_chunks.push(sse);
                }
            }
        }

        // Feed converted OpenAI chunks into StreamRelay
        let source: Pin<
            Box<dyn futures::Stream<Item = std::result::Result<Bytes, PrismStreamError>> + Send>,
        > = Box::pin(stream::iter(
            sse_chunks
                .into_iter()
                .map(|b| Ok(b) as std::result::Result<Bytes, PrismStreamError>),
        ));
        let (_relay, result_rx) = StreamRelay::start(source);
        let result = result_rx.await.unwrap();

        assert_eq!(result.model, "claude-3-5-sonnet");
        assert_eq!(result.completion_text, "Hello world");
        let usage = result.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
        assert!(result.ttft_ms.is_some());
    }
}
