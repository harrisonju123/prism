use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use chrono::Utc;
use uuid::Uuid;

use crate::error::{PrismError, Result};
use crate::keys::MaybeAuth;
use crate::keys::budget::BudgetCheckResult;
use crate::proxy::cost::compute_cost;
use crate::proxy::handler::{AppState, resolve_model};
use crate::types::{
    EventStatus, InferenceEvent, TaskType, TextCompletionChoice, TextCompletionRequest,
    TextCompletionResponse, Usage,
};

/// POST /v1/completions — OpenAI legacy text completions (used for FIM / edit predictions).
pub async fn text_completions(
    State(state): State<Arc<AppState>>,
    auth: MaybeAuth,
    Json(request): Json<TextCompletionRequest>,
) -> Result<Json<TextCompletionResponse>> {
    let start = Instant::now();
    let auth_ctx = auth.0;

    // --- Auth / rate limit / budget ---
    if let Some(ref ctx) = auth_ctx {
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
                let limit = ctx
                    .daily_budget_usd
                    .or(ctx.monthly_budget_usd)
                    .unwrap_or(0.0);
                return Err(PrismError::BudgetExceeded {
                    limit,
                    spent: limit,
                });
            }
            BudgetCheckResult::Warning { message } => {
                tracing::warn!(key_prefix = %ctx.key_prefix, %message, "budget warning");
            }
            BudgetCheckResult::Ok => {}
        }

        state.rate_limiter.record_request(&ctx.key_hash).await;
    }

    // --- Resolve provider ---
    let (provider_name, model_id) = resolve_model(&state.config, &request.model)?;

    let provider_cfg = state
        .config
        .providers
        .get(&provider_name)
        .ok_or_else(|| PrismError::ProviderNotConfigured(provider_name.clone()))?;

    let api_base = provider_cfg
        .api_base
        .as_deref()
        .unwrap_or("https://api.openai.com/v1");
    let api_key = provider_cfg.api_key.as_deref().unwrap_or("").to_string();

    // --- Forward request to provider's /completions endpoint ---
    let url = format!("{}/completions", api_base.trim_end_matches('/'));

    let backend_body = serde_json::json!({
        "model": model_id,
        "prompt": request.prompt,
        "max_tokens": request.max_tokens,
        "temperature": request.temperature,
        "stop": request.stop,
    });

    let http_resp = state
        .http_client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&backend_body)
        .send()
        .await
        .map_err(|e| PrismError::Internal(e.to_string()))?;

    let latency_ms = start.elapsed().as_millis() as u32;
    let status = http_resp.status();
    let body = http_resp
        .bytes()
        .await
        .map_err(|e| PrismError::Internal(e.to_string()))?;

    if !status.is_success() {
        return Err(PrismError::Provider(
            String::from_utf8_lossy(&body).to_string(),
        ));
    }

    let provider_resp: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| PrismError::Internal(e.to_string()))?;

    let text = provider_resp["choices"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let finish_reason = provider_resp["choices"][0]["finish_reason"]
        .as_str()
        .map(|s| s.to_string());
    let prompt_tokens = provider_resp["usage"]["prompt_tokens"]
        .as_u64()
        .unwrap_or(0) as u32;
    let completion_tokens = provider_resp["usage"]["completion_tokens"]
        .as_u64()
        .unwrap_or(0) as u32;
    let response_id = provider_resp["id"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    // --- Observability ---
    let usage = Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
        ..Default::default()
    };

    let event = InferenceEvent {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        provider: provider_name.clone(),
        model: model_id.clone(),
        status: EventStatus::Success,
        input_tokens: prompt_tokens,
        output_tokens: completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
        cache_read_input_tokens: 0,
        cache_creation_input_tokens: 0,
        estimated_cost_usd: compute_cost(&model_id, &usage),
        latency_ms,
        prompt_hash: String::new(),
        completion_hash: String::new(),
        task_type: Some(TaskType::FillInTheMiddle),
        routing_decision: None,
        variant_name: None,
        virtual_key_hash: auth_ctx.as_ref().map(|c| c.key_hash.clone()),
        team_id: auth_ctx.as_ref().and_then(|c| c.team_id.clone()),
        end_user_id: None,
        episode_id: None,
        metadata: "{}".to_string(),
        trace_id: None,
        span_id: None,
        parent_span_id: None,
        agent_framework: None,
        tool_calls_json: None,
        ttft_ms: None,
        session_id: None,
        provider_attempted: None,
    };
    let _ = state.event_tx.try_send(event);

    let created = Utc::now().timestamp() as u64;

    Ok(Json(TextCompletionResponse {
        id: response_id,
        object: "text_completion".to_string(),
        created,
        model: model_id,
        choices: vec![TextCompletionChoice {
            text,
            index: 0,
            finish_reason,
        }],
        usage,
    }))
}
