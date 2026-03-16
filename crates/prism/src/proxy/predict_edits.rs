use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{PrismError, Result};
use crate::keys::MaybeAuth;
use crate::keys::budget::BudgetCheckResult;
use crate::proxy::handler::{AppState, EventContext, build_event, resolve_model_with_fallbacks};
use crate::types::{ChatCompletionRequest, EventStatus, Message, MessageRole, ProviderResponse};

#[derive(Deserialize)]
pub struct EditPredictionRequest {
    pub model: String,
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    #[serde(default)]
    pub stop: Vec<String>,
}

#[derive(Serialize)]
pub struct EditPredictionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<EditPredictionChoice>,
    pub usage: EditPredictionUsage,
}

#[derive(Serialize)]
pub struct EditPredictionChoice {
    pub text: String,
    pub finish_reason: Option<String>,
}

#[derive(Serialize)]
pub struct EditPredictionUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// POST /v1/edit_predictions
pub async fn predict_edits(
    State(state): State<Arc<AppState>>,
    auth: MaybeAuth,
    Json(req): Json<EditPredictionRequest>,
) -> Result<Json<EditPredictionResponse>> {
    let start = Instant::now();
    let auth_ctx = auth.0;

    // --- Auth: model access control + rate limits + budget ---
    if let Some(ref ctx) = auth_ctx {
        if !ctx.allowed_models.is_empty() && !ctx.allowed_models.contains(&req.model) {
            return Err(PrismError::BadRequest(format!(
                "model '{}' not allowed for this key",
                req.model
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
            BudgetCheckResult::Exceeded {
                message,
                limit,
                spent,
            } => {
                tracing::warn!(key_prefix = %ctx.key_prefix, %message, "budget exceeded");
                return Err(PrismError::BudgetExceeded { limit, spent });
            }
            BudgetCheckResult::Warning { message } => {
                tracing::warn!(key_prefix = %ctx.key_prefix, %message, "budget warning");
            }
            BudgetCheckResult::Ok => {}
        }
        state.rate_limiter.record_request(&ctx.key_hash).await;
    }

    // --- Build chat completion request ---
    let stop_value = if req.stop.is_empty() {
        None
    } else {
        Some(serde_json::to_value(&req.stop).unwrap_or(serde_json::Value::Null))
    };

    let mut chat_req = ChatCompletionRequest {
        model: req.model.clone(),
        messages: vec![
            Message {
                role: MessageRole::System,
                content: Some(serde_json::Value::String(
                    "You are a code completion assistant. Complete the code at <fim_middle>. \
                     Output only the inserted text, nothing else."
                        .to_string(),
                )),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            },
            Message {
                role: MessageRole::User,
                content: Some(serde_json::Value::String(req.prompt.clone())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            },
        ],
        max_tokens: req.max_tokens,
        temperature: req.temperature.map(|t| t as f64),
        stop: stop_value,
        stream: false,
        ..Default::default()
    };

    // --- Routing ---
    if state.config.routing.enabled {
        let decision = crate::routing::resolve(
            crate::types::TaskType::CodeGeneration,
            1.0,
            &req.model,
            &state.fitness_cache,
            &state.routing_policy,
            state.config.routing.tier1_confidence_threshold,
            None,
        )
        .await;
        if let Some(decision) = decision {
            chat_req.model = decision.selected_model.clone();
        }
    }

    // --- Provider dispatch ---
    let (provider_name, model_id, _fallbacks) =
        resolve_model_with_fallbacks(&state.config, &chat_req.model)?;
    let provider = state.providers.get(&provider_name)?;

    tracing::info!(
        model = %chat_req.model,
        provider = %provider_name,
        "proxying edit prediction"
    );

    let provider_result = provider.chat_completion(&chat_req, &model_id).await?;
    let ProviderResponse::Complete(response) = provider_result else {
        return Err(PrismError::Internal(
            "expected non-streaming response from edit prediction".to_string(),
        ));
    };

    let latency_ms = start.elapsed().as_millis() as u32;
    let usage = response.usage.clone().unwrap_or_default();

    let completion_text = response
        .choices
        .first()
        .and_then(|c| c.message.content.as_ref())
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let prompt_hash = {
        let mut hasher = Sha256::new();
        hasher.update(req.prompt.as_bytes());
        hex::encode(hasher.finalize())
    };
    let completion_hash = {
        let mut hasher = Sha256::new();
        hasher.update(completion_text.as_bytes());
        hex::encode(hasher.finalize())
    };

    let event_ctx = EventContext {
        trace_id: None,
        span_id: None,
        parent_span_id: None,
        agent_framework: Some("edit_predictions".to_string()),
        tool_calls_json: None,
        ttft_ms: None,
        session_id: None,
        thread_id: None,
        provider_attempted: None,
    };
    let event = build_event(
        &provider_name,
        &response.model,
        EventStatus::Success,
        &usage,
        latency_ms,
        &prompt_hash,
        &completion_hash,
        Some(crate::types::TaskType::CodeGeneration),
        None,
        auth_ctx.as_ref(),
        None,
        Uuid::new_v4(),
        None,
        &event_ctx,
    );

    if let Some(ref ctx) = auth_ctx {
        state
            .rate_limiter
            .record_tokens(&ctx.key_hash, usage.total_tokens)
            .await;
        state
            .budget_tracker
            .record_spend(&ctx.key_hash, event.estimated_cost_usd);
    }

    state.session_cost_usd.fetch_add(
        (event.estimated_cost_usd * 1_000_000.0) as u64,
        std::sync::atomic::Ordering::Relaxed,
    );

    let _ = state.event_tx.send(event).await;

    let finish_reason = response
        .choices
        .first()
        .and_then(|c| c.finish_reason.clone());

    Ok(Json(EditPredictionResponse {
        id: response.id,
        object: "text_completion".to_string(),
        created: Utc::now().timestamp() as u64,
        model: response.model,
        choices: vec![EditPredictionChoice {
            text: completion_text,
            finish_reason,
        }],
        usage: EditPredictionUsage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        },
    }))
}
