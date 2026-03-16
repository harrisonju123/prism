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

/// Detect whether a completions request is a fill-in-the-middle (FIM) request.
/// FIM requests either have a `suffix` field or contain FIM special tokens in the prompt.
fn is_fim_request(request: &TextCompletionRequest) -> bool {
    if request.suffix.is_some() {
        return true;
    }
    let prompt = &request.prompt;
    prompt.contains("<fim_prefix>")
        || prompt.contains("<fim_suffix>")
        || prompt.contains("<fim_middle>")
        || prompt.contains("<|fim_prefix|>")
        || prompt.contains("<|fim_suffix|>")
        || prompt.contains("<|fim_middle|>")
}

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

    // --- FIM detection and fitness routing ---
    let fim_detected = is_fim_request(&request);
    let (resolved_model, routing_decision_str) = if fim_detected && state.config.routing.enabled {
        let decision = crate::routing::resolve(
            TaskType::FillInTheMiddle,
            1.0, // FIM detection is deterministic — maximum confidence
            &request.model,
            &state.fitness_cache,
            &state.routing_policy,
            state.config.routing.tier1_confidence_threshold,
            None,
        )
        .await;

        let decision_str = decision
            .as_ref()
            .map(|d| serde_json::to_string(d).unwrap_or_else(|_| format!("{:?}", d)));

        let selected = decision
            .as_ref()
            .map(|d| d.selected_model.clone())
            .unwrap_or_else(|| request.model.clone());

        tracing::info!(
            requested_model = %request.model,
            selected_model = %selected,
            routed = decision.is_some(),
            "FIM request routed via fitness scoring"
        );

        (selected, decision_str)
    } else {
        (request.model.clone(), None)
    };

    // --- Resolve provider ---
    let (provider_name, model_id) = resolve_model(&state.config, &resolved_model)?;

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

    let mut backend_body = serde_json::json!({
        "model": model_id,
        "prompt": request.prompt,
        "max_tokens": request.max_tokens,
        "temperature": request.temperature,
        "stop": request.stop,
    });
    if let Some(suffix) = &request.suffix {
        backend_body["suffix"] = serde_json::Value::String(suffix.clone());
    }

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

    let task_type = if fim_detected {
        Some(TaskType::FillInTheMiddle)
    } else {
        None
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
        task_type,
        routing_decision: routing_decision_str,
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
        thread_id: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TextCompletionRequest;

    fn make_request(prompt: &str, suffix: Option<&str>) -> TextCompletionRequest {
        TextCompletionRequest {
            model: "gpt-4o-mini".into(),
            prompt: prompt.into(),
            suffix: suffix.map(String::from),
            max_tokens: None,
            temperature: None,
            stop: vec![],
            extra: Default::default(),
        }
    }

    #[test]
    fn fim_detected_by_suffix_field() {
        let req = make_request("some code here", Some("// rest of function"));
        assert!(is_fim_request(&req));
    }

    #[test]
    fn fim_detected_by_fim_prefix_token() {
        let req = make_request(
            "<fim_prefix>def foo():<fim_suffix>    pass<fim_middle>",
            None,
        );
        assert!(is_fim_request(&req));
    }

    #[test]
    fn fim_detected_by_pipe_fim_tokens() {
        let req = make_request("<|fim_prefix|>let x = <|fim_suffix|>;<|fim_middle|>", None);
        assert!(is_fim_request(&req));
    }

    #[test]
    fn non_fim_request_not_detected() {
        let req = make_request("Write a function to sort a list", None);
        assert!(!is_fim_request(&req));
    }
}
