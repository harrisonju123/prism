use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{PrismError, Result};
use crate::keys::MaybeAuth;
use crate::keys::budget::BudgetCheckResult;
use crate::models;
use crate::proxy::cost::compute_cost;
use crate::proxy::handler::AppState;
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

    if request.stream {
        return Err(PrismError::BadRequest(
            "streaming via /v1/messages not yet supported — use /v1/chat/completions".into(),
        ));
    }

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

    // Convert to ChatCompletionRequest, use existing provider infrastructure
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
        completion_hash: String::new(),
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
        tool_calls_json: None,
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
        messages.push(crate::types::Message {
            role: msg.role.clone(),
            content: Some(msg.content.clone()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        });
    }

    crate::types::ChatCompletionRequest {
        model: req.model.clone(),
        messages,
        temperature: req.temperature,
        top_p: req.top_p,
        max_tokens: Some(req.max_tokens),
        stream: false,
        stream_options: None,
        stop: None,
        tools: None,
        tool_choice: None,
        response_format: None,
        user: None,
        extra: Default::default(),
    }
}

fn to_anthropic_response(resp: &crate::types::ChatCompletionResponse) -> AnthropicMessagesResponse {
    let text = resp
        .choices
        .first()
        .and_then(|c| c.message.content.as_ref())
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default();

    let usage = resp.usage.as_ref();

    AnthropicMessagesResponse {
        id: resp.id.clone(),
        response_type: "message".into(),
        role: "assistant".into(),
        content: vec![ContentBlock {
            block_type: "text".into(),
            text: Some(text),
        }],
        model: resp.model.clone(),
        stop_reason: resp.choices.first().and_then(|c| c.finish_reason.clone()),
        usage: AnthropicUsage {
            input_tokens: usage.map(|u| u.prompt_tokens).unwrap_or(0),
            output_tokens: usage.map(|u| u.completion_tokens).unwrap_or(0),
            cache_read_input_tokens: usage.map(|u| u.cache_read_input_tokens).unwrap_or(0),
            cache_creation_input_tokens: usage.map(|u| u.cache_creation_input_tokens).unwrap_or(0),
        },
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
                finish_reason: Some("end_turn".into()),
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
    }
}
