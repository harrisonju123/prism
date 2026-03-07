use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, Message as BedrockMessage, StopReason, SystemContentBlock,
};

use crate::error::{PrismError, Result};
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, Choice, EmbeddingRequest, EmbeddingResponse,
    Message, PrismStreamError, ProviderResponse, Usage,
};

use super::Provider;

pub struct BedrockProvider {
    client: Client,
}

impl BedrockProvider {
    pub async fn new(region: Option<String>) -> Self {
        let mut config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest());

        if let Some(r) = region {
            config_loader = config_loader.region(aws_types::region::Region::new(r));
        }

        let sdk_config = config_loader.load().await;
        let client = Client::new(&sdk_config);

        Self { client }
    }
}

// ---------------------------------------------------------------------------
// Format conversion: OpenAI <-> Bedrock Converse API
// ---------------------------------------------------------------------------

fn to_bedrock_role(role: &str) -> ConversationRole {
    match role {
        "assistant" => ConversationRole::Assistant,
        _ => ConversationRole::User,
    }
}

fn map_stop_reason(reason: &StopReason) -> &'static str {
    match reason {
        StopReason::EndTurn => "stop",
        StopReason::MaxTokens => "length",
        StopReason::ToolUse => "tool_calls",
        StopReason::ContentFiltered => "content_filter",
        _ => "stop",
    }
}

fn extract_messages(req: &ChatCompletionRequest) -> (Vec<SystemContentBlock>, Vec<BedrockMessage>) {
    let mut system_prompts: Vec<SystemContentBlock> = Vec::new();
    let mut messages: Vec<BedrockMessage> = Vec::new();

    for msg in &req.messages {
        if msg.role == "system" {
            if let Some(content) = &msg.content {
                let text = content_value_to_string(content);
                system_prompts.push(SystemContentBlock::Text(text));
            }
        } else if msg.role == "tool" {
            if let Some(content) = &msg.content {
                let text = content_value_to_string(content);
                let bedrock_msg = BedrockMessage::builder()
                    .role(ConversationRole::User)
                    .content(ContentBlock::Text(text))
                    .build()
                    .expect("valid message");
                messages.push(bedrock_msg);
            }
        } else {
            let role = to_bedrock_role(&msg.role);
            let text = msg
                .content
                .as_ref()
                .map(content_value_to_string)
                .unwrap_or_default();
            let bedrock_msg = BedrockMessage::builder()
                .role(role)
                .content(ContentBlock::Text(text))
                .build()
                .expect("valid message");
            messages.push(bedrock_msg);
        }
    }

    (system_prompts, messages)
}

fn build_inference_config(
    req: &ChatCompletionRequest,
) -> Option<aws_sdk_bedrockruntime::types::InferenceConfiguration> {
    if req.temperature.is_none() && req.top_p.is_none() && req.max_tokens.is_none() {
        return None;
    }
    let mut builder = aws_sdk_bedrockruntime::types::InferenceConfiguration::builder();
    if let Some(t) = req.temperature {
        builder = builder.temperature(t as f32);
    }
    if let Some(tp) = req.top_p {
        builder = builder.top_p(tp as f32);
    }
    if let Some(mt) = req.max_tokens {
        builder = builder.max_tokens(mt as i32);
    }
    Some(builder.build())
}

fn from_converse_output(
    output: aws_sdk_bedrockruntime::operation::converse::ConverseOutput,
    model_id: &str,
) -> ChatCompletionResponse {
    let mut text = String::new();
    if let Some(bedrock_output) = output.output() {
        if let aws_sdk_bedrockruntime::types::ConverseOutput::Message(msg) = bedrock_output {
            for block in msg.content() {
                if let ContentBlock::Text(t) = block {
                    text.push_str(t);
                }
            }
        }
    }

    let stop_reason = map_stop_reason(output.stop_reason()).to_string();

    let (input_tokens, output_tokens) = output
        .usage
        .as_ref()
        .map(|u| (u.input_tokens() as u32, u.output_tokens() as u32))
        .unwrap_or((0, 0));

    ChatCompletionResponse {
        id: format!("bedrock-{}", uuid::Uuid::new_v4()),
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
            finish_reason: Some(stop_reason),
        }],
        usage: Some(Usage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens + output_tokens,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        }),
        extra: Default::default(),
    }
}

fn content_value_to_string(value: &serde_json::Value) -> String {
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

fn stream_event_to_sse_bytes(
    event: &aws_sdk_bedrockruntime::types::ConverseStreamOutput,
    model_id: &str,
    request_id: &str,
) -> Option<bytes::Bytes> {
    match event {
        aws_sdk_bedrockruntime::types::ConverseStreamOutput::ContentBlockDelta(delta) => {
            if let Some(aws_sdk_bedrockruntime::types::ContentBlockDelta::Text(text)) =
                delta.delta()
            {
                let chunk = serde_json::json!({
                    "id": request_id,
                    "object": "chat.completion.chunk",
                    "created": chrono::Utc::now().timestamp(),
                    "model": model_id,
                    "choices": [{
                        "index": 0,
                        "delta": { "content": text },
                        "finish_reason": null,
                    }],
                });
                let line = format!("data: {}\n\n", serde_json::to_string(&chunk).ok()?);
                Some(bytes::Bytes::from(line))
            } else {
                None
            }
        }
        aws_sdk_bedrockruntime::types::ConverseStreamOutput::MessageStop(stop) => {
            let finish_reason = map_stop_reason(stop.stop_reason());

            let chunk = serde_json::json!({
                "id": request_id,
                "object": "chat.completion.chunk",
                "created": chrono::Utc::now().timestamp(),
                "model": model_id,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": finish_reason,
                }],
            });
            let line = format!("data: {}\n\n", serde_json::to_string(&chunk).ok()?);
            Some(bytes::Bytes::from(line))
        }
        aws_sdk_bedrockruntime::types::ConverseStreamOutput::Metadata(meta) => {
            if let Some(usage) = meta.usage() {
                let chunk = serde_json::json!({
                    "id": request_id,
                    "object": "chat.completion.chunk",
                    "created": chrono::Utc::now().timestamp(),
                    "model": model_id,
                    "choices": [],
                    "usage": {
                        "prompt_tokens": usage.input_tokens(),
                        "completion_tokens": usage.output_tokens(),
                        "total_tokens": usage.input_tokens() + usage.output_tokens(),
                    },
                });
                let mut lines = format!("data: {}\n\n", serde_json::to_string(&chunk).ok()?);
                lines.push_str("data: [DONE]\n\n");
                Some(bytes::Bytes::from(lines))
            } else {
                None
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Provider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for BedrockProvider {
    fn name(&self) -> &'static str {
        "bedrock"
    }

    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
        model_id: &str,
    ) -> Result<ProviderResponse> {
        let (system_prompts, messages) = extract_messages(request);
        let inference_config = build_inference_config(request);

        if request.stream {
            let mut builder = self.client.converse_stream().model_id(model_id);
            for sys in system_prompts {
                builder = builder.system(sys);
            }
            for msg in messages {
                builder = builder.messages(msg);
            }
            if let Some(ic) = inference_config {
                builder = builder.inference_config(ic);
            }

            let stream_output = builder
                .send()
                .await
                .map_err(|e| PrismError::Provider(format!("Bedrock ConverseStream error: {e}")))?;

            let request_id = format!("bedrock-{}", uuid::Uuid::new_v4());
            let model_owned = model_id.to_string();

            let mut event_stream = stream_output.stream;
            let stream = async_stream::try_stream! {
                loop {
                    match event_stream.recv().await {
                        Ok(Some(event)) => {
                            if let Some(chunk_bytes) =
                                stream_event_to_sse_bytes(&event, &model_owned, &request_id)
                            {
                                yield chunk_bytes;
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            Err(PrismStreamError::Other(format!(
                                "Bedrock stream error: {e}"
                            )))?;
                        }
                    }
                }
            };

            Ok(ProviderResponse::Stream(Box::pin(stream)))
        } else {
            let mut builder = self.client.converse().model_id(model_id);
            for sys in system_prompts {
                builder = builder.system(sys);
            }
            for msg in messages {
                builder = builder.messages(msg);
            }
            if let Some(ic) = inference_config {
                builder = builder.inference_config(ic);
            }

            let output = builder
                .send()
                .await
                .map_err(|e| PrismError::Provider(format!("Bedrock Converse error: {e}")))?;

            Ok(ProviderResponse::Complete(from_converse_output(
                output, model_id,
            )))
        }
    }

    async fn embed(
        &self,
        _request: &EmbeddingRequest,
        _model_id: &str,
    ) -> Result<EmbeddingResponse> {
        Err(PrismError::BadRequest(
            "embeddings not supported on Bedrock".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_value_to_string_plain() {
        let val = serde_json::Value::String("hello world".into());
        assert_eq!(content_value_to_string(&val), "hello world");
    }

    #[test]
    fn test_content_value_to_string_array() {
        let val = serde_json::json!([
            {"type": "text", "text": "Hello"},
            {"type": "text", "text": "World"},
        ]);
        assert_eq!(content_value_to_string(&val), "Hello\nWorld");
    }

    #[test]
    fn test_to_bedrock_role() {
        assert_eq!(to_bedrock_role("assistant"), ConversationRole::Assistant);
        assert_eq!(to_bedrock_role("user"), ConversationRole::User);
        assert_eq!(to_bedrock_role("anything_else"), ConversationRole::User);
    }

    #[test]
    fn test_map_stop_reason() {
        assert_eq!(map_stop_reason(&StopReason::EndTurn), "stop");
        assert_eq!(map_stop_reason(&StopReason::MaxTokens), "length");
        assert_eq!(map_stop_reason(&StopReason::ToolUse), "tool_calls");
        assert_eq!(
            map_stop_reason(&StopReason::ContentFiltered),
            "content_filter"
        );
    }

    #[test]
    fn test_from_converse_output_basic() {
        let output = aws_sdk_bedrockruntime::operation::converse::ConverseOutput::builder()
            .output(aws_sdk_bedrockruntime::types::ConverseOutput::Message(
                BedrockMessage::builder()
                    .role(ConversationRole::Assistant)
                    .content(ContentBlock::Text("test response".into()))
                    .build()
                    .unwrap(),
            ))
            .stop_reason(StopReason::EndTurn)
            .usage(
                aws_sdk_bedrockruntime::types::TokenUsage::builder()
                    .input_tokens(10)
                    .output_tokens(5)
                    .total_tokens(15)
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();

        let response = from_converse_output(output, "anthropic.claude-3-haiku-20240307-v1:0");

        assert_eq!(response.object, "chat.completion");
        assert_eq!(response.model, "anthropic.claude-3-haiku-20240307-v1:0");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.choices[0].message.role, "assistant");
        assert_eq!(
            response.choices[0].message.content,
            Some(serde_json::Value::String("test response".into()))
        );
        assert_eq!(response.choices[0].finish_reason, Some("stop".to_string()));
        let usage = response.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn test_stream_event_to_sse_content_delta() {
        let event = aws_sdk_bedrockruntime::types::ConverseStreamOutput::ContentBlockDelta(
            aws_sdk_bedrockruntime::types::ContentBlockDeltaEvent::builder()
                .content_block_index(0)
                .delta(aws_sdk_bedrockruntime::types::ContentBlockDelta::Text(
                    "hello".into(),
                ))
                .build()
                .unwrap(),
        );

        let result = stream_event_to_sse_bytes(&event, "test-model", "req-123");
        assert!(result.is_some());
        let bytes = result.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.starts_with("data: "));
        assert!(text.contains("\"content\":\"hello\""));
        assert!(text.contains("chat.completion.chunk"));
    }

    #[test]
    fn test_extract_messages_with_system() {
        let req = ChatCompletionRequest {
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

        let (system_prompts, messages) = extract_messages(&req);
        assert_eq!(system_prompts.len(), 1);
        assert_eq!(messages.len(), 1);
    }
}
