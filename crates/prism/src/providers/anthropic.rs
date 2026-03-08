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
    /// System prompt — serialized as a JSON array of content blocks with cache_control
    /// for prompt caching support, or as a plain string.
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
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
    // tool_use fields
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
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

fn openai_to_anthropic_tool_choice(tc: &serde_json::Value) -> serde_json::Value {
    match tc {
        serde_json::Value::String(s) => match s.as_str() {
            "auto" | "none" => serde_json::json!({"type": "auto"}),
            "required" => serde_json::json!({"type": "any"}),
            _ => tc.clone(),
        },
        _ if tc.get("type").and_then(|t| t.as_str()) == Some("function") => {
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            serde_json::json!({"type": "tool", "name": name})
        }
        _ => tc.clone(),
    }
}

fn to_anthropic_request(req: &ChatCompletionRequest, model_id: &str) -> AnthropicRequest {
    let mut system = None;
    let mut messages = Vec::new();

    for msg in &req.messages {
        if msg.role == "system" {
            // Anthropic uses a top-level system field.
            // Format as content block array with cache_control for prompt caching.
            if let Some(content) = &msg.content {
                let text = content_to_string(content);
                system = Some(serde_json::json!([{
                    "type": "text",
                    "text": text,
                    "cache_control": {"type": "ephemeral"}
                }]));
            }
        } else if msg.role == "tool" {
            // OpenAI tool result → Anthropic tool_result block in a user turn
            messages.push(AnthropicMessage {
                role: "user".into(),
                content: serde_json::json!([{
                    "type": "tool_result",
                    "tool_use_id": msg.tool_call_id.as_deref().unwrap_or(""),
                    "content": msg.content.as_ref()
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                }]),
            });
        } else if msg.role == "assistant" && msg.tool_calls.is_some() {
            // OpenAI assistant with tool_calls → Anthropic tool_use blocks
            let calls = msg.tool_calls.as_ref().unwrap();
            let mut blocks: Vec<serde_json::Value> = Vec::new();
            // Prepend text content if present
            if let Some(ref c) = msg.content {
                let t = content_to_string(c);
                if !t.is_empty() {
                    blocks.push(serde_json::json!({"type": "text", "text": t}));
                }
            }
            for tc in calls {
                let args_obj: serde_json::Value = tc["function"]["arguments"]
                    .as_str()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(serde_json::json!({}));
                blocks.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc["id"].as_str().unwrap_or(""),
                    "name": tc["function"]["name"].as_str().unwrap_or(""),
                    "input": args_obj,
                }));
            }
            messages.push(AnthropicMessage {
                role: "assistant".into(),
                content: serde_json::Value::Array(blocks),
            });
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

    let tool_choice = req.tool_choice.as_ref().map(openai_to_anthropic_tool_choice);

    AnthropicRequest {
        model: model_id.to_string(),
        max_tokens: req.max_tokens.unwrap_or(4096),
        messages,
        system,
        temperature: req.temperature,
        top_p: req.top_p,
        stream: if req.stream { Some(true) } else { None },
        tools,
        tool_choice,
    }
}

fn from_anthropic_response(resp: AnthropicResponse) -> ChatCompletionResponse {
    let text = resp
        .content
        .iter()
        .filter_map(|b| b.text.as_deref())
        .collect::<Vec<_>>()
        .join("");

    let tool_calls: Vec<serde_json::Value> = resp
        .content
        .iter()
        .filter(|b| b.r#type == "tool_use")
        .map(|b| {
            serde_json::json!({
                "id": b.id.as_deref().unwrap_or(""),
                "type": "function",
                "function": {
                    "name": b.name.as_deref().unwrap_or(""),
                    "arguments": b.input.as_ref()
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

    ChatCompletionResponse {
        id: resp.id,
        object: "chat.completion".into(),
        created: chrono::Utc::now().timestamp(),
        model: resp.model,
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: "assistant".into(),
                content,
                name: None,
                tool_calls: tool_calls_opt,
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
            .header("anthropic-beta", "prompt-caching-2024-07-31")
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
                    id: None,
                    name: None,
                    input: None,
                },
                ContentBlock {
                    r#type: "text".into(),
                    text: Some("world!".into()),
                    id: None,
                    name: None,
                    input: None,
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

    #[test]
    fn test_tool_result_message_conversion() {
        let req = ChatCompletionRequest {
            model: "claude-3".into(),
            messages: vec![
                Message {
                    role: "assistant".into(),
                    content: None,
                    name: None,
                    tool_calls: Some(vec![serde_json::json!({
                        "id": "toolu_abc",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"city\":\"London\"}"}
                    })]),
                    tool_call_id: None,
                    extra: Default::default(),
                },
                Message {
                    role: "tool".into(),
                    content: Some(serde_json::Value::String("Sunny, 22°C".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: Some("toolu_abc".into()),
                    extra: Default::default(),
                },
            ],
            ..Default::default()
        };

        let anthropic_req = to_anthropic_request(&req, "claude-3");
        // First message: assistant with tool_use block
        // Second message: user with tool_result block
        assert_eq!(anthropic_req.messages.len(), 2);
        assert_eq!(anthropic_req.messages[1].role, "user");
        let content = &anthropic_req.messages[1].content;
        let arr = content.as_array().expect("should be array");
        assert_eq!(arr[0]["type"], "tool_result");
        assert_eq!(arr[0]["tool_use_id"], "toolu_abc");
        assert_eq!(arr[0]["content"], "Sunny, 22°C");
    }

    #[test]
    fn test_assistant_tool_calls_conversion() {
        let req = ChatCompletionRequest {
            model: "claude-3".into(),
            messages: vec![Message {
                role: "assistant".into(),
                content: Some(serde_json::Value::String("Let me check.".into())),
                name: None,
                tool_calls: Some(vec![serde_json::json!({
                    "id": "toolu_xyz",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}
                })]),
                tool_call_id: None,
                extra: Default::default(),
            }],
            ..Default::default()
        };

        let anthropic_req = to_anthropic_request(&req, "claude-3");
        assert_eq!(anthropic_req.messages.len(), 1);
        assert_eq!(anthropic_req.messages[0].role, "assistant");
        let blocks = anthropic_req.messages[0]
            .content
            .as_array()
            .expect("should be array");
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "Let me check.");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["id"], "toolu_xyz");
        assert_eq!(blocks[1]["name"], "get_weather");
        assert_eq!(blocks[1]["input"]["city"], "Paris");
    }

    #[test]
    fn test_tool_choice_conversion() {
        assert_eq!(
            openai_to_anthropic_tool_choice(&serde_json::Value::String("auto".into())),
            serde_json::json!({"type": "auto"})
        );
        assert_eq!(
            openai_to_anthropic_tool_choice(&serde_json::Value::String("required".into())),
            serde_json::json!({"type": "any"})
        );
        let specific = serde_json::json!({"type": "function", "function": {"name": "get_weather"}});
        let result = openai_to_anthropic_tool_choice(&specific);
        assert_eq!(result["type"], "tool");
        assert_eq!(result["name"], "get_weather");
    }

    #[test]
    fn test_tool_use_response_maps_to_tool_calls() {
        let resp = AnthropicResponse {
            id: "msg_tool".into(),
            model: "claude-3-5-sonnet".into(),
            content: vec![ContentBlock {
                r#type: "tool_use".into(),
                text: None,
                id: Some("toolu_abc123".into()),
                name: Some("get_weather".into()),
                input: Some(serde_json::json!({"city": "London"})),
            }],
            stop_reason: Some("tool_use".into()),
            usage: AnthropicUsage {
                input_tokens: 20,
                output_tokens: 10,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        };

        let oai = from_anthropic_response(resp);
        assert_eq!(oai.choices[0].finish_reason.as_deref(), Some("tool_calls"));
        // content must be null when tool_calls present
        assert!(oai.choices[0].message.content.is_none());
        let tool_calls = oai.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "toolu_abc123");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
    }

    #[test]
    fn test_tool_use_input_serialized_to_string() {
        let resp = AnthropicResponse {
            id: "msg_args".into(),
            model: "claude-3-5-sonnet".into(),
            content: vec![ContentBlock {
                r#type: "tool_use".into(),
                text: None,
                id: Some("toolu_xyz".into()),
                name: Some("get_weather".into()),
                input: Some(serde_json::json!({"city": "London"})),
            }],
            stop_reason: Some("tool_use".into()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        };

        let oai = from_anthropic_response(resp);
        let tool_calls = oai.choices[0].message.tool_calls.as_ref().unwrap();
        let args_str = tool_calls[0]["function"]["arguments"].as_str().unwrap();
        let args_val: serde_json::Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(args_val["city"], "London");
    }

    #[test]
    fn test_mixed_text_and_tool_use_response() {
        let resp = AnthropicResponse {
            id: "msg_mixed".into(),
            model: "claude-3-5-sonnet".into(),
            content: vec![
                ContentBlock {
                    r#type: "text".into(),
                    text: Some("Let me check the weather.".into()),
                    id: None,
                    name: None,
                    input: None,
                },
                ContentBlock {
                    r#type: "tool_use".into(),
                    text: None,
                    id: Some("toolu_weather".into()),
                    name: Some("get_weather".into()),
                    input: Some(serde_json::json!({"city": "Paris"})),
                },
            ],
            stop_reason: Some("tool_use".into()),
            usage: AnthropicUsage {
                input_tokens: 15,
                output_tokens: 8,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        };

        let oai = from_anthropic_response(resp);
        // When tool_calls present, content should be null per OpenAI spec
        assert!(oai.choices[0].message.content.is_none());
        let tool_calls = oai.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
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
