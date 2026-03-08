use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{PrismError, Result};
use crate::keys::MaybeAuth;
use crate::keys::budget::BudgetCheckResult;
use crate::models;
use crate::proxy::cost::compute_cost;
use crate::proxy::handler::AppState;
use crate::proxy::streaming::StreamRelay;
use crate::types::{EventStatus, InferenceEvent, Usage};

// ---------------------------------------------------------------------------
// Anthropic-native request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessagesResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub response_type: String,
    pub role: String,
    pub content: Vec<ContentBlock>,
    pub model: String,
    pub stop_reason: Option<String>,
    pub usage: AnthropicUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// POST /v1/messages — Anthropic-native messages endpoint.
pub async fn anthropic_messages(
    State(state): State<Arc<AppState>>,
    auth: MaybeAuth,
    headers: HeaderMap,
    Json(request): Json<AnthropicMessagesRequest>,
) -> Result<Response> {
    let start = Instant::now();
    let request_model = request.model.clone();
    let auth_ctx = auth.0;

    // Extract trace headers
    let trace_id = headers
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let span_id = headers
        .get("x-span-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let session_id = headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let episode_id = headers
        .get("x-episode-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok());

    // End user from header
    let end_user_id = headers
        .get("x-end-user-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // --- Auth, rate limiting, budget ---
    let (key_hash, team_id) = if let Some(ref ctx) = auth_ctx {
        if !ctx.allowed_models.is_empty() && !ctx.allowed_models.contains(&request_model) {
            return Err(PrismError::BadRequest(format!(
                "model '{}' not allowed for this key",
                request_model
            )));
        }
        if let Some(rpm_limit) = ctx.rpm_limit {
            let result = state.rate_limiter.check_rpm(&ctx.key_hash, rpm_limit).await;
            if !result.allowed {
                return Err(PrismError::RateLimited {
                    retry_after_secs: result.retry_after_secs,
                });
            }
        }

        let budget_result = state.budget_tracker.check(
            &ctx.key_hash,
            ctx.daily_budget_usd,
            ctx.monthly_budget_usd,
            ctx.budget_action,
        );
        match budget_result {
            BudgetCheckResult::Exceeded { message } => {
                tracing::warn!(key_prefix = %ctx.key_prefix, %message, "budget exceeded");
                return Err(PrismError::BudgetExceeded);
            }
            BudgetCheckResult::Warning { message } => {
                tracing::warn!(key_prefix = %ctx.key_prefix, %message, "budget warning");
            }
            BudgetCheckResult::Ok => {}
        }

        state.rate_limiter.record_request(&ctx.key_hash).await;

        (Some(ctx.key_hash.clone()), ctx.team_id.clone())
    } else {
        (None, None)
    };

    // --- Resolve model to provider ---
    let model_entry = models::lookup_model(&request_model);
    let (provider_name, model_id) = if let Some(entry) = model_entry {
        (entry.provider.to_string(), entry.model_id.to_string())
    } else {
        let parts: Vec<&str> = request_model.splitn(2, '/').collect();
        if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            return Err(PrismError::ModelNotFound(format!(
                "unknown model: {}",
                request_model
            )));
        }
    };

    let provider = state.providers.get(&provider_name)?;

    // --- Streaming path ---
    if request.stream {
        let mut chat_request = to_chat_completion_request(&request);
        chat_request.stream = true;
        // Inject stream_options so OpenAI-compatible providers include usage in the final chunk.
        if chat_request.stream_options.is_none() {
            chat_request.stream_options = Some(crate::types::StreamOptions {
                include_usage: true,
            });
        }

        let provider_response = provider.chat_completion(&chat_request, &model_id).await?;
        let raw_stream = match provider_response {
            crate::types::ProviderResponse::Stream(s) => s,
            _ => return Err(PrismError::Internal("expected stream from provider".into())),
        };

        // Wrap raw_stream with StreamRelay so we can capture final usage/completion
        // for token accounting after the stream ends.
        let (relay, result_rx) = StreamRelay::start(raw_stream);

        let msg_id = format!("msg_{}", Uuid::new_v4().simple());
        let anthropic_stream = openai_sse_to_anthropic_sse(relay, msg_id, model_id.clone());

        let sse_stream = anthropic_stream.flat_map(|item| {
            let events: Vec<std::result::Result<Event, std::convert::Infallible>> = match item {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    text.split("\n\n")
                        .filter(|block| !block.trim().is_empty())
                        .filter_map(|block| {
                            let mut event_type: Option<String> = None;
                            let mut data_str: Option<String> = None;
                            for line in block.lines() {
                                if let Some(et) = line.strip_prefix("event: ") {
                                    event_type = Some(et.to_string());
                                } else if let Some(d) = line.strip_prefix("data: ") {
                                    data_str = Some(d.to_string());
                                }
                            }
                            if let Some(data) = data_str {
                                let mut ev = Event::default().data(data);
                                if let Some(et) = event_type {
                                    ev = ev.event(et);
                                }
                                Some(Ok(ev))
                            } else {
                                None
                            }
                        })
                        .collect()
                }
                Err(_) => vec![],
            };
            futures::stream::iter(events)
        });

        // Spawn a background task to capture usage and emit the inference event once
        // the stream finishes.
        {
            let event_tx = state.event_tx.clone();
            let budget_tracker = state.budget_tracker.clone();
            let rate_limiter = state.rate_limiter.clone();
            let request_model_owned = request_model.clone();
            let provider_name_owned = provider_name.clone();
            let key_hash_owned = key_hash.clone();
            let team_id_owned = team_id.clone();
            let end_user_id_owned = end_user_id.clone();
            let episode_id_owned = episode_id;
            let trace_id_owned = trace_id.clone();
            let span_id_owned = span_id.clone();
            let session_id_owned = session_id.clone();
            let session_cost_usd = state.session_cost_usd.clone();
            let start_clone = start;
            // Build prompt hash from request messages
            let prompt_hash = {
                let mut hasher = Sha256::new();
                for msg in &request.messages {
                    hasher.update(msg.role.as_bytes());
                    hasher.update(msg.content.to_string().as_bytes());
                }
                hex::encode(hasher.finalize())
            };
            tokio::spawn(async move {
                if let Ok(stream_result) = result_rx.await {
                    let latency_ms = start_clone.elapsed().as_millis() as u32;
                    let usage = stream_result.usage.unwrap_or_default();
                    let cost_usage = Usage {
                        prompt_tokens: usage.prompt_tokens,
                        completion_tokens: usage.completion_tokens,
                        total_tokens: usage.total_tokens,
                        cache_read_input_tokens: usage.cache_read_input_tokens,
                        cache_creation_input_tokens: usage.cache_creation_input_tokens,
                    };
                    let cost = compute_cost(&request_model_owned, &cost_usage);

                    if let Some(ref kh) = key_hash_owned {
                        budget_tracker.record_spend(kh, cost);
                        rate_limiter
                            .record_tokens(kh, cost_usage.total_tokens)
                            .await;
                    }

                    let completion_hash = {
                        if stream_result.completion_text.is_empty() {
                            String::new()
                        } else {
                            let mut hasher = Sha256::new();
                            hasher.update(stream_result.completion_text.as_bytes());
                            hex::encode(hasher.finalize())
                        }
                    };

                    let input_tokens = cost_usage.prompt_tokens;
                    let output_tokens = cost_usage.completion_tokens;
                    let event = InferenceEvent {
                        id: Uuid::new_v4(),
                        timestamp: Utc::now(),
                        provider: provider_name_owned,
                        model: request_model_owned.clone(),
                        status: EventStatus::Success,
                        input_tokens,
                        output_tokens,
                        total_tokens: cost_usage.total_tokens,
                        cache_read_input_tokens: cost_usage.cache_read_input_tokens,
                        cache_creation_input_tokens: cost_usage.cache_creation_input_tokens,
                        estimated_cost_usd: cost,
                        latency_ms,
                        prompt_hash,
                        completion_hash,
                        task_type: None,
                        routing_decision: Some("anthropic_native".into()),
                        variant_name: None,
                        virtual_key_hash: key_hash_owned,
                        team_id: team_id_owned,
                        end_user_id: end_user_id_owned,
                        episode_id: episode_id_owned,
                        metadata: String::new(),
                        trace_id: trace_id_owned,
                        span_id: span_id_owned,
                        parent_span_id: None,
                        agent_framework: None,
                        tool_calls_json: None,
                        ttft_ms: stream_result.ttft_ms,
                        session_id: session_id_owned,
                        provider_attempted: None,
                    };

                    let event_cost = event.estimated_cost_usd;
                    let _ = event_tx.send(event).await;

                    session_cost_usd.fetch_add(
                        (event_cost * 1_000_000.0) as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );

                    tracing::info!(
                        model = %request_model_owned,
                        input_tokens,
                        output_tokens,
                        latency_ms,
                        "anthropic native stream completed"
                    );
                }
            });
        }


        return Ok(Sse::new(sse_stream)
            .keep_alive(KeepAlive::default())
            .into_response());
    }

    // --- Non-streaming path ---
    let chat_request = to_chat_completion_request(&request);
    let provider_response = provider.chat_completion(&chat_request, &model_id).await?;

    let chat_response = match provider_response {
        crate::types::ProviderResponse::Complete(resp) => resp,
        crate::types::ProviderResponse::Stream(_) => {
            return Err(PrismError::Internal("unexpected stream response".into()));
        }
    };

    // Extract token counts
    let usage = chat_response.usage.as_ref();
    let input_tokens = usage.map(|u| u.prompt_tokens).unwrap_or(0);
    let output_tokens = usage.map(|u| u.completion_tokens).unwrap_or(0);
    let cache_read = usage.map(|u| u.cache_read_input_tokens).unwrap_or(0);
    let cache_creation = usage.map(|u| u.cache_creation_input_tokens).unwrap_or(0);

    let anthropic_response = to_anthropic_response(&chat_response);

    let latency_ms = start.elapsed().as_millis() as u32;
    let cost_usage = Usage {
        prompt_tokens: input_tokens,
        completion_tokens: output_tokens,
        total_tokens: input_tokens + output_tokens,
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: cache_creation,
    };
    let cost = compute_cost(&request_model, &cost_usage);

    if let Some(ref kh) = key_hash {
        state.budget_tracker.record_spend(kh, cost);
        // Record token usage for TPM rate limiting
        state
            .rate_limiter
            .record_tokens(kh, cost_usage.total_tokens)
            .await;
    }

    // Build prompt hash
    let prompt_hash = {
        let mut hasher = Sha256::new();
        for msg in &request.messages {
            hasher.update(msg.role.as_bytes());
            hasher.update(msg.content.to_string().as_bytes());
        }
        hex::encode(hasher.finalize())
    };

    // Compute completion hash from text blocks in response
    let completion_hash = {
        let text: String = anthropic_response
            .content
            .iter()
            .filter_map(|block| block.text.as_deref())
            .collect();
        if text.is_empty() {
            String::new()
        } else {
            let mut hasher = Sha256::new();
            hasher.update(text.as_bytes());
            hex::encode(hasher.finalize())
        }
    };

    // Extract tool_calls_json for observability
    let tool_calls_json = request
        .tools
        .as_ref()
        .and_then(|tools| serde_json::to_string(tools).ok());

    // Emit inference event
    let event = InferenceEvent {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        provider: provider_name.to_string(),
        model: request_model,
        status: EventStatus::Success,
        input_tokens,
        output_tokens,
        total_tokens: input_tokens + output_tokens,
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: cache_creation,
        estimated_cost_usd: cost,
        latency_ms,
        prompt_hash,
        completion_hash,
        task_type: None,
        routing_decision: Some("anthropic_native".into()),
        variant_name: None,
        virtual_key_hash: key_hash.clone(),
        team_id,
        end_user_id,
        episode_id,
        metadata: String::new(),
        trace_id,
        span_id,
        parent_span_id: None,
        agent_framework: None,
        tool_calls_json,
        ttft_ms: None,
        session_id,
        provider_attempted: None,
    };

    let _ = state.event_tx.try_send(event);

    tracing::info!(
        model = %anthropic_response.model,
        input_tokens,
        output_tokens,
        latency_ms,
        "anthropic native request completed"
    );

    Ok(Json(anthropic_response).into_response())
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

fn anthropic_to_openai_tool_choice(tc: &serde_json::Value) -> serde_json::Value {
    match tc.get("type").and_then(|t| t.as_str()) {
        Some("auto") => serde_json::Value::String("auto".into()),
        Some("any") => serde_json::Value::String("required".into()),
        Some("tool") => serde_json::json!({
            "type": "function",
            "function": {"name": tc["name"].as_str().unwrap_or("")}
        }),
        _ => tc.clone(),
    }
}

fn openai_to_anthropic_stop_reason(reason: &str) -> &str {
    match reason {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        other => other,
    }
}

fn to_chat_completion_request(
    req: &AnthropicMessagesRequest,
) -> crate::types::ChatCompletionRequest {
    let mut messages = Vec::new();

    if let Some(ref system) = req.system {
        messages.push(crate::types::Message {
            role: "system".into(),
            content: Some(serde_json::Value::String(system.clone())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        });
    }

    for msg in &req.messages {
        match &msg.content {
            serde_json::Value::Array(blocks) => {
                let has_tool_result = blocks
                    .iter()
                    .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"));
                let has_tool_use = blocks
                    .iter()
                    .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"));

                if has_tool_result {
                    // User turn returning tool results — emit one role:"tool" msg per result
                    for block in blocks {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                            let tool_use_id = block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            // content may be a string or array of text blocks
                            let content_str = match block.get("content") {
                                Some(serde_json::Value::String(s)) => s.clone(),
                                Some(serde_json::Value::Array(arr)) => arr
                                    .iter()
                                    .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join(""),
                                _ => String::new(),
                            };
                            messages.push(crate::types::Message {
                                role: "tool".into(),
                                content: Some(serde_json::Value::String(content_str)),
                                name: None,
                                tool_calls: None,
                                tool_call_id: Some(tool_use_id),
                                extra: Default::default(),
                            });
                        }
                    }
                    // Any text blocks alongside tool results become a separate user message
                    let text_parts: Vec<&str> = blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                        .collect();
                    if !text_parts.is_empty() {
                        messages.push(crate::types::Message {
                            role: "user".into(),
                            content: Some(serde_json::Value::String(text_parts.join("\n"))),
                            name: None,
                            tool_calls: None,
                            tool_call_id: None,
                            extra: Default::default(),
                        });
                    }
                } else if has_tool_use {
                    // Assistant turn that called tools
                    let text_parts: Vec<&str> = blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                        .collect();
                    let content = if text_parts.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::String(text_parts.join("")))
                    };

                    let tool_calls: Vec<serde_json::Value> = blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                        .map(|b| {
                            let args_str = b
                                .get("input")
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "{}".to_string());
                            serde_json::json!({
                                "id": b.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                                "type": "function",
                                "function": {
                                    "name": b.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                                    "arguments": args_str,
                                }
                            })
                        })
                        .collect();

                    messages.push(crate::types::Message {
                        role: "assistant".into(),
                        content,
                        name: None,
                        tool_calls: Some(tool_calls),
                        tool_call_id: None,
                        extra: Default::default(),
                    });
                } else {
                    // Text-only array — join and pass through
                    let text: String = blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("");
                    messages.push(crate::types::Message {
                        role: msg.role.clone(),
                        content: Some(serde_json::Value::String(text)),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                        extra: Default::default(),
                    });
                }
            }
            other => {
                // String or other — pass through as-is
                messages.push(crate::types::Message {
                    role: msg.role.clone(),
                    content: Some(other.clone()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                });
            }
        }
    }

    // Convert Anthropic tools → OpenAI format
    let tools = req.tools.as_ref().and_then(|tools| {
        let converted: Vec<crate::types::Tool> = tools
            .iter()
            .filter_map(|t| {
                Some(crate::types::Tool {
                    r#type: "function".into(),
                    function: crate::types::ToolFunction {
                        name: t["name"].as_str()?.to_string(),
                        description: t["description"].as_str().map(String::from),
                        parameters: t.get("input_schema").cloned(),
                    },
                })
            })
            .collect();
        if converted.is_empty() {
            None
        } else {
            Some(converted)
        }
    });

    // Convert Anthropic tool_choice → OpenAI format
    let tool_choice = req
        .tool_choice
        .as_ref()
        .map(anthropic_to_openai_tool_choice);

    crate::types::ChatCompletionRequest {
        model: req.model.clone(),
        messages,
        temperature: req.temperature,
        top_p: req.top_p,
        max_tokens: Some(req.max_tokens),
        stream: false,
        stream_options: None,
        stop: None,
        tools,
        tool_choice,
        response_format: None,
        user: None,
        extra: Default::default(),
    }
}

fn to_anthropic_response(resp: &crate::types::ChatCompletionResponse) -> AnthropicMessagesResponse {
    let choice = resp.choices.first();
    let tool_calls = choice.and_then(|c| c.message.tool_calls.as_ref());

    let (content, stop_reason) = if let Some(calls) = tool_calls {
        let blocks = calls
            .iter()
            .map(|tc| ContentBlock {
                block_type: "tool_use".into(),
                text: None,
                id: tc.get("id").and_then(|v| v.as_str()).map(String::from),
                name: tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                    .map(String::from),
                input: tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok()),
            })
            .collect();
        (blocks, Some("tool_use".into()))
    } else {
        let text = choice
            .and_then(|c| c.message.content.as_ref())
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        let sr = choice
            .and_then(|c| c.finish_reason.as_deref())
            .map(openai_to_anthropic_stop_reason)
            .map(String::from);
        (
            vec![ContentBlock {
                block_type: "text".into(),
                text: Some(text),
                id: None,
                name: None,
                input: None,
            }],
            sr,
        )
    };

    let usage = resp.usage.as_ref();

    AnthropicMessagesResponse {
        id: resp.id.clone(),
        response_type: "message".into(),
        role: "assistant".into(),
        content,
        model: resp.model.clone(),
        stop_reason,
        usage: AnthropicUsage {
            input_tokens: usage.map(|u| u.prompt_tokens).unwrap_or(0),
            output_tokens: usage.map(|u| u.completion_tokens).unwrap_or(0),
            cache_read_input_tokens: usage.map(|u| u.cache_read_input_tokens).unwrap_or(0),
            cache_creation_input_tokens: usage.map(|u| u.cache_creation_input_tokens).unwrap_or(0),
        },
    }
}

// ---------------------------------------------------------------------------
// Streaming: OpenAI SSE → Anthropic SSE converter
// ---------------------------------------------------------------------------

/// Convert a StreamRelay into a pinned stream compatible with openai_sse_to_anthropic_sse.
fn relay_to_stream(
    relay: StreamRelay,
) -> std::pin::Pin<
    Box<
        dyn futures::Stream<
                Item = std::result::Result<bytes::Bytes, crate::types::PrismStreamError>,
            > + Send,
    >,
> {
    Box::pin(relay)
}

fn format_sse(event_type: &str, data: &serde_json::Value) -> bytes::Bytes {
    let json = serde_json::to_string(data).unwrap_or_default();
    bytes::Bytes::from(format!("event: {event_type}\ndata: {json}\n\n"))
}

fn openai_sse_to_anthropic_sse(
    stream: impl futures::Stream<
        Item = std::result::Result<bytes::Bytes, crate::types::PrismStreamError>,
    > + Send
    + 'static,
    message_id: String,
    model: String,
) -> impl futures::Stream<Item = std::result::Result<bytes::Bytes, crate::types::PrismStreamError>>
+ Send
+ 'static {
    async_stream::try_stream! {
        let mut pinned = Box::pin(stream);
        let mut buffer = String::new();
        let mut emitted_preamble = false;
        let mut block_index: u32 = 0;
        let mut block_type = "text".to_string();
        let mut tool_block_map: HashMap<u32, u32> = HashMap::new();
        let mut done = false;
        // Track real token counts from OpenAI usage chunks.
        // input_tokens: reported in message_start; populated from the usage chunk when available.
        // output_tokens: reported in message_delta at stream end.
        // Cache tokens are accounted in the StreamRelay (real accounting), not in the SSE output.
        let mut input_tokens: u32 = 0;
        let mut output_tokens: u32 = 0;

        while let Some(chunk) = pinned.next().await {
            if done { break; }
            let bytes = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            loop {
                let Some(pos) = buffer.find('\n') else { break; };
                let line = buffer[..pos].trim_end_matches('\r').to_string();
                buffer.drain(..pos + 1);

                let data = match line.strip_prefix("data: ") {
                    Some(d) => d.to_string(),
                    None => continue,
                };

                if data == "[DONE]" {
                    done = true;
                    break;
                }

                let chunk_val: serde_json::Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Capture usage from any chunk that includes it (OpenAI sends usage in the
                // final chunk when stream_options.include_usage is set, or the AnthropicProvider
                // embeds it in the MessageDelta-derived chunk).
                if let Some(usage_val) = chunk_val.get("usage") {
                    if let Some(pt) = usage_val.get("prompt_tokens").and_then(|v| v.as_u64()) {
                        input_tokens = pt as u32;
                    }
                    if let Some(ct) = usage_val.get("completion_tokens").and_then(|v| v.as_u64()) {
                        output_tokens = ct as u32;
                    }
                }

                let delta = &chunk_val["choices"][0]["delta"];
                let finish_reason = chunk_val["choices"][0]["finish_reason"]
                    .as_str()
                    .filter(|s| !s.is_empty());

                // Emit preamble on first chunk with role field
                if !emitted_preamble && delta.get("role").is_some() {
                    let msg_start = serde_json::json!({
                        "type": "message_start",
                        "message": {
                            "id": message_id,
                            "type": "message",
                            "role": "assistant",
                            "content": [],
                            "model": model,
                            "stop_reason": null,
                            "stop_sequence": null,
                            "usage": {"input_tokens": input_tokens, "output_tokens": 0}
                        }
                    });
                    yield format_sse("message_start", &msg_start);

                    let block_start = serde_json::json!({
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": {"type": "text", "text": ""}
                    });
                    yield format_sse("content_block_start", &block_start);

                    emitted_preamble = true;
                    block_index = 0;
                    block_type = "text".to_string();
                }

                // Text content delta
                if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                    if !text.is_empty() {
                        if block_type != "text" {
                            yield format_sse("content_block_stop", &serde_json::json!({
                                "type": "content_block_stop",
                                "index": block_index
                            }));
                            block_index += 1;
                            block_type = "text".to_string();
                            yield format_sse("content_block_start", &serde_json::json!({
                                "type": "content_block_start",
                                "index": block_index,
                                "content_block": {"type": "text", "text": ""}
                            }));
                        }
                        yield format_sse("content_block_delta", &serde_json::json!({
                            "type": "content_block_delta",
                            "index": block_index,
                            "delta": {"type": "text_delta", "text": text}
                        }));
                    }
                }

                // Tool call deltas
                if let Some(calls) = delta.get("tool_calls").and_then(|tc| tc.as_array()) {
                    for tc in calls {
                        let oai_index = tc["index"].as_u64().unwrap_or(0) as u32;

                        // New tool call: has id field
                        if tc.get("id").and_then(|v| v.as_str()).is_some() {
                            yield format_sse("content_block_stop", &serde_json::json!({
                                "type": "content_block_stop",
                                "index": block_index
                            }));
                            block_index += 1;
                            tool_block_map.insert(oai_index, block_index);
                            block_type = "tool_use".to_string();

                            yield format_sse("content_block_start", &serde_json::json!({
                                "type": "content_block_start",
                                "index": block_index,
                                "content_block": {
                                    "type": "tool_use",
                                    "id": tc["id"].as_str().unwrap_or(""),
                                    "name": tc["function"]["name"].as_str().unwrap_or(""),
                                    "input": {}
                                }
                            }));
                        }

                        // Argument streaming
                        if let Some(args) = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|v| v.as_str())
                        {
                            if !args.is_empty() {
                                let idx = tool_block_map
                                    .get(&oai_index)
                                    .copied()
                                    .unwrap_or(block_index);
                                yield format_sse("content_block_delta", &serde_json::json!({
                                    "type": "content_block_delta",
                                    "index": idx,
                                    "delta": {"type": "input_json_delta", "partial_json": args}
                                }));
                            }
                        }
                    }
                }

                // Finish reason — close block and send message_delta + message_stop
                if let Some(reason) = finish_reason {
                    yield format_sse("content_block_stop", &serde_json::json!({
                        "type": "content_block_stop",
                        "index": block_index
                    }));
                    let anthropic_reason = openai_to_anthropic_stop_reason(reason);
                    yield format_sse("message_delta", &serde_json::json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": anthropic_reason, "stop_sequence": null},
                        "usage": {"output_tokens": output_tokens}
                    }));
                    yield format_sse("message_stop", &serde_json::json!({"type": "message_stop"}));
                    done = true;
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_chat_completion_request() {
        let req = AnthropicMessagesRequest {
            model: "claude-3-opus".into(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".into(),
                content: serde_json::Value::String("Hello".into()),
            }],
            system: Some("You are helpful.".into()),
            temperature: Some(0.7),
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
            metadata: None,
        };

        let chat_req = to_chat_completion_request(&req);
        assert_eq!(chat_req.model, "claude-3-opus");
        assert_eq!(chat_req.messages.len(), 2); // system + user
        assert_eq!(chat_req.messages[0].role, "system");
        assert_eq!(chat_req.messages[1].role, "user");
        assert_eq!(chat_req.max_tokens, Some(1024));
    }

    #[test]
    fn test_to_anthropic_response() {
        let chat_resp = crate::types::ChatCompletionResponse {
            id: "msg_123".into(),
            object: "chat.completion".into(),
            created: 1234567890,
            model: "claude-3-opus".into(),
            choices: vec![crate::types::Choice {
                index: 0,
                message: crate::types::Message {
                    role: "assistant".into(),
                    content: Some(serde_json::Value::String("Hi there!".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(crate::types::Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                ..Default::default()
            }),
            extra: Default::default(),
        };

        let resp = to_anthropic_response(&chat_resp);
        assert_eq!(resp.id, "msg_123");
        assert_eq!(resp.response_type, "message");
        assert_eq!(resp.role, "assistant");
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.content[0].text.as_deref(), Some("Hi there!"));
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn test_request_deserialization() {
        let json = r#"{
            "model": "claude-3-opus",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "Hello"}]
        }"#;
        let req: AnthropicMessagesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "claude-3-opus");
        assert!(!req.stream);
        assert!(req.system.is_none());
    }

    #[test]
    fn test_response_serialization() {
        let resp = AnthropicMessagesResponse {
            id: "msg_1".into(),
            response_type: "message".into(),
            role: "assistant".into(),
            content: vec![ContentBlock {
                block_type: "text".into(),
                text: Some("Hello!".into()),
                id: None,
                name: None,
                input: None,
            }],
            model: "claude-3-opus".into(),
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["type"], "message");
        assert_eq!(json["content"][0]["type"], "text");
        // id/name/input should be absent (skip_serializing_if = None)
        assert!(json["content"][0].get("id").is_none() || json["content"][0]["id"].is_null());
    }

    #[test]
    fn test_tool_round_trip_through_conversion() {
        let req = AnthropicMessagesRequest {
            model: "claude-3-5-sonnet".into(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".into(),
                content: serde_json::Value::String("What is the weather in London?".into()),
            }],
            system: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: Some(vec![serde_json::json!({
                "name": "get_weather",
                "description": "Get weather for a city",
                "input_schema": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"]
                }
            })]),
            tool_choice: None,
            metadata: None,
        };

        let chat_req = to_chat_completion_request(&req);
        let tools = chat_req.tools.expect("tools should be present");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].r#type, "function");
        assert_eq!(tools[0].function.name, "get_weather");
        assert_eq!(
            tools[0].function.description.as_deref(),
            Some("Get weather for a city")
        );
        assert!(tools[0].function.parameters.is_some());
    }

    #[test]
    fn test_tool_use_message_converted_to_tool_calls() {
        let req = AnthropicMessagesRequest {
            model: "claude-3-5-sonnet".into(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "assistant".into(),
                content: serde_json::json!([
                    {"type": "text", "text": "Let me check."},
                    {
                        "type": "tool_use",
                        "id": "toolu_abc",
                        "name": "get_weather",
                        "input": {"city": "London"}
                    }
                ]),
            }],
            system: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
            metadata: None,
        };

        let chat_req = to_chat_completion_request(&req);
        assert_eq!(chat_req.messages.len(), 1);
        let msg = &chat_req.messages[0];
        assert_eq!(msg.role, "assistant");
        assert_eq!(
            msg.content.as_ref().and_then(|c| c.as_str()),
            Some("Let me check.")
        );
        let calls = msg
            .tool_calls
            .as_ref()
            .expect("tool_calls should be present");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["id"], "toolu_abc");
        assert_eq!(calls[0]["type"], "function");
        assert_eq!(calls[0]["function"]["name"], "get_weather");
    }

    #[test]
    fn test_tool_result_message_converted_to_role_tool() {
        let req = AnthropicMessagesRequest {
            model: "claude-3-5-sonnet".into(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".into(),
                content: serde_json::json!([{
                    "type": "tool_result",
                    "tool_use_id": "toolu_abc",
                    "content": "Sunny, 22°C"
                }]),
            }],
            system: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
            metadata: None,
        };

        let chat_req = to_chat_completion_request(&req);
        assert_eq!(chat_req.messages.len(), 1);
        let msg = &chat_req.messages[0];
        assert_eq!(msg.role, "tool");
        assert_eq!(msg.tool_call_id.as_deref(), Some("toolu_abc"));
        assert_eq!(
            msg.content.as_ref().and_then(|c| c.as_str()),
            Some("Sunny, 22°C")
        );
    }

    #[test]
    fn test_to_anthropic_response_tool_calls() {
        let chat_resp = crate::types::ChatCompletionResponse {
            id: "msg_tool".into(),
            object: "chat.completion".into(),
            created: 1234567890,
            model: "claude-3-5-sonnet".into(),
            choices: vec![crate::types::Choice {
                index: 0,
                message: crate::types::Message {
                    role: "assistant".into(),
                    content: None,
                    name: None,
                    tool_calls: Some(vec![serde_json::json!({
                        "id": "toolu_xyz",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"London\"}"
                        }
                    })]),
                    tool_call_id: None,
                    extra: Default::default(),
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: Some(crate::types::Usage {
                prompt_tokens: 20,
                completion_tokens: 10,
                total_tokens: 30,
                ..Default::default()
            }),
            extra: Default::default(),
        };

        let resp = to_anthropic_response(&chat_resp);
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.content[0].block_type, "tool_use");
        assert_eq!(resp.content[0].id.as_deref(), Some("toolu_xyz"));
        assert_eq!(resp.content[0].name.as_deref(), Some("get_weather"));
        let input = resp.content[0]
            .input
            .as_ref()
            .expect("input should be present");
        assert_eq!(input["city"], "London");
    }

    #[test]
    fn test_tool_choice_conversion() {
        let auto = serde_json::json!({"type": "auto"});
        let result = anthropic_to_openai_tool_choice(&auto);
        assert_eq!(result, serde_json::Value::String("auto".into()));

        let any = serde_json::json!({"type": "any"});
        let result = anthropic_to_openai_tool_choice(&any);
        assert_eq!(result, serde_json::Value::String("required".into()));

        let specific = serde_json::json!({"type": "tool", "name": "get_weather"});
        let result = anthropic_to_openai_tool_choice(&specific);
        assert_eq!(result["type"], "function");
        assert_eq!(result["function"]["name"], "get_weather");
    }

    #[test]
    fn test_stop_reason_conversion() {
        assert_eq!(openai_to_anthropic_stop_reason("stop"), "end_turn");
        assert_eq!(openai_to_anthropic_stop_reason("length"), "max_tokens");
        assert_eq!(openai_to_anthropic_stop_reason("tool_calls"), "tool_use");
        assert_eq!(openai_to_anthropic_stop_reason("other"), "other");
    }
}
