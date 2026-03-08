use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use rand::Rng;

use crate::cache::ResponseCache;
use crate::classifier::RulesClassifier;
use crate::classifier::taxonomy::{ClassifierInput, OutputFormatHint};
use crate::config::Config;
use crate::error::{PrismError, Result};
use crate::experiment::engine::ExperimentEngine;
use crate::experiment::feedback::FeedbackEvent;
use crate::keys::audit::AuditService;
use crate::keys::budget::{BudgetCheckResult, BudgetTracker};
use crate::keys::rate_limit::RateLimiter;
use crate::keys::{AuthContext, KeyService, MaybeAuth};
use crate::mcp::extractor::extract_mcp_calls;
use crate::mcp::types::McpCall;
use crate::models;
use crate::models::alias::{AliasCache, AliasRepository};
use crate::providers::ProviderRegistry;
use crate::providers::health::ProviderHealthTracker;
use crate::proxy::cost::compute_cost;
use crate::proxy::streaming::StreamRelay;
use crate::routing::FitnessCache;
use crate::routing::types::RoutingPolicy;
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest, EventStatus, InferenceEvent,
    ProviderResponse, Usage,
};

/// POST /v1/chat/completions
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    auth: MaybeAuth,
    headers: HeaderMap,
    Json(mut request): Json<ChatCompletionRequest>,
) -> Result<Response> {
    let start = Instant::now();
    let request_model = request.model.clone();
    let auth_ctx = auth.0;

    // --- Extract episode, cache, and trace headers ---
    let episode_id = headers
        .get("x-episode-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(Uuid::new_v4);

    let no_cache = headers
        .get("x-no-cache")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

    // Trace context: extract from standard headers or generate
    let trace_id = headers
        .get("x-trace-id")
        .or_else(|| headers.get("traceparent"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let span_id = headers
        .get("x-span-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let parent_span_id = headers
        .get("x-parent-span-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Session tracking header
    let session_id = headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Agent framework detection from headers or user-agent
    let agent_framework = headers
        .get("x-agent-framework")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| detect_agent_framework(&headers));

    // --- Auth: model access control + rate limits + budget ---
    if let Some(ref ctx) = auth_ctx {
        // Model access control
        if !ctx.allowed_models.is_empty() && !ctx.allowed_models.contains(&request_model) {
            return Err(PrismError::BadRequest(format!(
                "model '{}' not allowed for this key",
                request_model
            )));
        }

        // RPM check
        if let Some(rpm_limit) = ctx.rpm_limit {
            let result = state.rate_limiter.check_rpm(&ctx.key_hash, rpm_limit).await;
            if !result.allowed {
                if let Some(ref m) = state.metrics {
                    m.record_rate_limited();
                }
                return Err(PrismError::RateLimited {
                    retry_after_secs: result.retry_after_secs,
                });
            }
        }

        // TPM check (pre-request estimate not available, skip pre-check for TPM)

        // Budget check
        let budget_result = state.budget_tracker.check(
            &ctx.key_hash,
            ctx.daily_budget_usd,
            ctx.monthly_budget_usd,
            ctx.budget_action,
        );
        match budget_result {
            BudgetCheckResult::Exceeded { message, limit, spent } => {
                tracing::warn!(key_prefix = %ctx.key_prefix, %message, "budget exceeded");
                return Err(PrismError::BudgetExceeded { limit, spent });
            }
            BudgetCheckResult::Warning { message } => {
                tracing::warn!(key_prefix = %ctx.key_prefix, %message, "budget warning");
                // Continue but log
            }
            BudgetCheckResult::Ok => {}
        }

        // Record RPM request (after checks pass)
        state.rate_limiter.record_request(&ctx.key_hash).await;
    }

    // --- Classification + Routing ---
    let (task_type, routing_decision_str) = if state.config.routing.enabled {
        let input = build_classifier_input(&request);
        let mut classification = RulesClassifier::classify(&input);

        tracing::debug!(
            task_type = %classification.task_type,
            confidence = classification.confidence,
            signals = ?classification.signals,
            "classified request"
        );

        // Embedding tier: if rules confidence is below threshold, try local embedding classifier
        let emb_cfg = &state.config.routing.embedding_classifier;
        if classification.confidence < state.config.routing.classifier_confidence_threshold
            && emb_cfg.enabled
        {
            let emb = crate::classifier::EmbeddingClassifier::get().classify(&input);
            if emb.confidence > classification.confidence {
                tracing::debug!(
                    rules_task = %classification.task_type,
                    rules_confidence = classification.confidence,
                    emb_task = %emb.task_type,
                    emb_confidence = emb.confidence,
                    "embedding classifier improved on rules"
                );
                classification = emb;
            }
        }

        // LLM fallback: if rules confidence is below threshold and LLM classifier is enabled
        let llm_cfg = &state.config.routing.llm_classifier;
        if classification.confidence < state.config.routing.classifier_confidence_threshold
            && llm_cfg.enabled
        {
            let (llm_provider_name, _) =
                resolve_model(&state.config, &llm_cfg.model).unwrap_or_default();
            if let Ok(llm_provider) = state.providers.get(&llm_provider_name) {
                match crate::classifier::llm_fallback::llm_classify(
                    &input,
                    llm_provider,
                    &llm_cfg.model,
                    llm_cfg.timeout_ms,
                )
                .await
                {
                    Ok(llm_result) => {
                        tracing::info!(
                            rules_task = %classification.task_type,
                            rules_confidence = classification.confidence,
                            llm_task = %llm_result.task_type,
                            llm_confidence = llm_result.confidence,
                            "LLM classifier fallback used"
                        );
                        classification = llm_result;
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "LLM classifier fallback failed, using rules result"
                        );
                    }
                }
            }
        }

        let decision =
            if classification.confidence >= state.config.routing.classifier_confidence_threshold {
                crate::routing::resolve(
                    classification.task_type,
                    classification.confidence,
                    &request_model,
                    &state.fitness_cache,
                    &state.routing_policy,
                    state.config.routing.tier1_confidence_threshold,
                )
                .await
            } else {
                None
            };

        let decision_str = decision
            .as_ref()
            .map(|d| serde_json::to_string(d).unwrap_or_else(|_| format!("{:?}", d)));

        (Some(classification.task_type), decision_str)
    } else {
        (None, None)
    };

    // --- Session tracking ---
    if let (Some(sid), Some(tt)) = (&session_id, task_type) {
        let mut tracker = state.session_tracker.lock().await;
        let phase = tracker.record(sid, tt);
        tracing::debug!(session_id = %sid, phase = ?phase, "session phase detected");
    }

    // --- Experiment: find matching experiment and select variant ---
    let variant_name = if let Some(ref engine) = state.experiment_engine {
        let mut selected: Option<crate::experiment::engine::VariantSelection> = None;
        for (exp_name, exp) in &state.config.experiments.experiments {
            if exp.function_name == request_model
                && let Some(sel) = engine.select_variant(exp, exp_name, episode_id)
            {
                selected = Some(sel);
                break;
            }
        }
        if let Some(ref sel) = selected {
            // Override request fields from variant
            request.model = sel.variant.model.clone();
            if let Some(temp) = sel.variant.temperature {
                request.temperature = Some(temp);
            }
            if let Some(mt) = sel.variant.max_tokens {
                request.max_tokens = Some(mt);
            }
            if let Some(ref prefix) = sel.variant.system_prompt_prefix {
                prepend_system_prompt(&mut request.messages, prefix);
            }
            tracing::info!(
                experiment = %sel.experiment_name,
                variant = %sel.variant.name,
                "experiment variant selected"
            );
            Some(sel.variant.name.clone())
        } else {
            None
        }
    } else {
        None
    };

    // Resolve aliases (DB cache first, then static) before routing
    // Skip if the model name is already a config-defined model — config takes priority
    if !state.config.models.contains_key(&request.model) {
        if let Some(resolved) = models::resolve_alias_cached(
            &request.model,
            state.alias_cache.as_deref(),
        )
        .await
        {
            tracing::debug!(alias = %request.model, resolved = %resolved, "model alias resolved");
            request.model = resolved;
        }
    }

    // Resolve which provider and model_id to use (with fallbacks)
    let (primary_provider_name, primary_model_id, fallback_providers) =
        resolve_model_with_fallbacks(&state.config, &request.model)?;
    let provider = state.providers.get(&primary_provider_name)?;
    let mut provider_name = primary_provider_name.clone();
    let model_id = primary_model_id.clone();

    tracing::info!(
        model = %request.model,
        provider = %provider_name,
        fallbacks = fallback_providers.len(),
        stream = request.stream,
        task_type = ?task_type,
        "proxying chat completion"
    );

    // --- Context window management ---
    if state.config.context_management.enabled {
        let ctx_window = models::lookup_model(&request.model)
            .map(|m| m.context_window)
            .or_else(|| {
                state
                    .config
                    .models
                    .get(&request.model)
                    .and_then(|mc| mc.context_window)
            });
        if let Some(window) = ctx_window {
            let reserve = state.config.context_management.response_reserve_tokens;
            let budget = window.saturating_sub(reserve);
            let current = crate::proxy::context_window::estimate_messages_tokens(&request.messages);
            if current > budget {
                match state.config.context_management.strategy.as_str() {
                    "error" => {
                        return Err(PrismError::BadRequest(format!(
                            "estimated prompt tokens ({current}) exceeds context window budget ({budget})"
                        )));
                    }
                    _ => {
                        let dropped = crate::proxy::context_window::truncate_to_fit(
                            &mut request.messages,
                            budget,
                        );
                        tracing::warn!(
                            dropped,
                            model = %request.model,
                            "truncated messages to fit context window"
                        );
                    }
                }
            }
        }
    }

    // --- Dry-run: return routing metadata without calling provider ---
    if headers
        .get("x-prism-dry-run")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
    {
        let fallback_chain: Vec<serde_json::Value> = fallback_providers
            .iter()
            .map(|(p, m)| serde_json::json!({"provider": p, "model": m}))
            .collect();
        return Ok(Json(serde_json::json!({
            "dry_run": true,
            "routed_model": request.model,
            "routed_provider": provider_name,
            "task_type": task_type.map(|t| t.to_string()),
            "fallback_chain": fallback_chain,
            "routing_decision": routing_decision_str,
        }))
        .into_response());
    }

    // Hash the prompt for observability (never store raw content)
    let prompt_hash = hash_messages(&request.messages);

    // Extract tool calls JSON for observability
    let tool_calls_json = extract_tool_calls_json(&request);

    // Pre-compute cache key for reuse in check + store
    let cache_key = if state.response_cache.is_some() && !no_cache {
        Some(ResponseCache::cache_key(&request))
    } else {
        None
    };

    // --- Cache check ---
    if let Some(ref cache) = state.response_cache
        && let Some(ref cache_key) = cache_key
        && !request.stream
    {
        if let Some(cached_response) = cache.get(&cache_key).await {
            let latency_ms = start.elapsed().as_millis() as u32;
            let usage = cached_response.usage.clone().unwrap_or_default();
            let completion_hash = hash_completion(&cached_response);

            let event_ctx = EventContext {
                trace_id: trace_id.clone(),
                span_id: span_id.clone(),
                parent_span_id: parent_span_id.clone(),
                agent_framework: agent_framework.clone(),
                tool_calls_json: tool_calls_json.clone(),
                ttft_ms: None,
                session_id: session_id.clone(),
                provider_attempted: None,
            };
            let event = build_event(
                &provider_name,
                &cached_response.model,
                EventStatus::Success,
                &usage,
                latency_ms,
                &prompt_hash,
                &completion_hash,
                task_type,
                routing_decision_str.clone(),
                auth_ctx.as_ref(),
                variant_name.clone(),
                episode_id,
                request.user.clone(),
                &event_ctx,
            );
            let _ = state.event_tx.send(event).await;

            // Record metrics for cache hit
            if let Some(ref m) = state.metrics {
                m.record_request(&request_model, latency_ms as u64, false);
                m.record_tokens(usage.total_tokens as u64);
                m.record_cache_hit();
            }

            tracing::info!(cache = "HIT", "returning cached response");

            let mut response = Json(&cached_response).into_response();
            response
                .headers_mut()
                .insert("x-cache", "HIT".parse().unwrap());
            return Ok(response);
        }
    }

    // Strip tools/tool_choice for models that don't support them
    let model_supports_tools = state
        .config
        .models
        .get(&request_model)
        .map(|mc| mc.supports_tools)
        .or_else(|| models::lookup_model(&request_model).map(|m| m.supports_tools))
        .unwrap_or(true);
    if !model_supports_tools && request.tools.is_some() {
        tracing::debug!(model = %request_model, "stripping unsupported tools param");
        request.tools = None;
        request.tool_choice = None;
    }

    // Inject stream_options to request usage data in the final streaming chunk
    if request.stream && request.stream_options.is_none() {
        request.stream_options = Some(crate::types::StreamOptions {
            include_usage: true,
        });
    }

    // --- Provider call with retry + failover ---
    let retry_config = &state.config.retry;
    let provider_result = if request.stream {
        // Streaming path: try primary, then fallbacks on error
        match provider.chat_completion(&request, &model_id).await {
            Ok(resp) => resp,
            Err(primary_err) if primary_err.is_retryable() && !fallback_providers.is_empty() => {
                tracing::warn!(
                    provider = %provider_name,
                    error = %primary_err,
                    "streaming primary provider failed, trying fallbacks"
                );

                let mut last_err = primary_err;
                let mut fallback_result = None;

                for (fb_provider_name, fb_model_id) in &fallback_providers {
                    // Skip degraded providers
                    if let Some(ref ht) = state.health_tracker {
                        if !ht.is_available(fb_provider_name) {
                            tracing::warn!(
                                fallback_provider = %fb_provider_name,
                                "streaming fallback provider degraded, skipping"
                            );
                            continue;
                        }
                    }

                    let fb_provider = match state.providers.get(fb_provider_name) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(
                                fallback_provider = %fb_provider_name,
                                error = %e,
                                "streaming fallback provider not available, skipping"
                            );
                            continue;
                        }
                    };

                    tracing::info!(
                        fallback_provider = %fb_provider_name,
                        fallback_model = %fb_model_id,
                        "attempting streaming fallback provider"
                    );

                    match fb_provider.chat_completion(&request, fb_model_id).await {
                        Ok(resp) => {
                            provider_name = fb_provider_name.clone();
                            if let Some(ref ht) = state.health_tracker {
                                ht.record_success(fb_provider_name);
                            }
                            tracing::info!(
                                fallback_provider = %fb_provider_name,
                                "streaming failover succeeded"
                            );
                            fallback_result = Some(resp);
                            break;
                        }
                        Err(e) => {
                            if let Some(ref ht) = state.health_tracker {
                                ht.record_failure(fb_provider_name, e.to_string());
                            }
                            tracing::warn!(
                                fallback_provider = %fb_provider_name,
                                error = %e,
                                "streaming fallback provider failed"
                            );
                            last_err = e;
                        }
                    }
                }

                if let Some(result) = fallback_result {
                    result
                } else {
                    return Err(last_err);
                }
            }
            Err(e) => return Err(e),
        }
    } else {
        let primary_result = crate::proxy::retry::with_retry(retry_config, || {
            let req = &request;
            let mid = &model_id;
            let prov = &provider;
            async move { prov.chat_completion(req, mid).await }
        })
        .await;

        match primary_result {
            Ok(resp) => resp,
            Err(primary_err) if primary_err.is_retryable() && !fallback_providers.is_empty() => {
                tracing::warn!(
                    provider = %provider_name,
                    error = %primary_err,
                    "primary provider exhausted retries, trying fallbacks"
                );

                let mut last_err = primary_err;
                let mut succeeded = false;
                let mut fallback_result = None;

                for (fb_provider_name, fb_model_id) in &fallback_providers {
                    // Skip degraded providers
                    if let Some(ref ht) = state.health_tracker {
                        if !ht.is_available(fb_provider_name) {
                            tracing::warn!(
                                fallback_provider = %fb_provider_name,
                                "fallback provider degraded, skipping"
                            );
                            continue;
                        }
                    }

                    let fb_provider = match state.providers.get(fb_provider_name) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(
                                fallback_provider = %fb_provider_name,
                                error = %e,
                                "fallback provider not available, skipping"
                            );
                            continue;
                        }
                    };

                    tracing::info!(
                        fallback_provider = %fb_provider_name,
                        fallback_model = %fb_model_id,
                        "attempting fallback provider"
                    );

                    match crate::proxy::retry::with_retry(retry_config, || {
                        let req = &request;
                        let mid = fb_model_id;
                        let prov = &fb_provider;
                        async move { prov.chat_completion(req, mid).await }
                    })
                    .await
                    {
                        Ok(resp) => {
                            provider_name = fb_provider_name.clone();
                            if let Some(ref ht) = state.health_tracker {
                                ht.record_success(fb_provider_name);
                            }
                            tracing::info!(
                                fallback_provider = %fb_provider_name,
                                "failover succeeded"
                            );
                            fallback_result = Some(resp);
                            succeeded = true;
                            break;
                        }
                        Err(e) => {
                            if let Some(ref ht) = state.health_tracker {
                                ht.record_failure(fb_provider_name, e.to_string());
                            }
                            tracing::warn!(
                                fallback_provider = %fb_provider_name,
                                error = %e,
                                "fallback provider failed"
                            );
                            last_err = e;
                        }
                    }
                }

                if succeeded {
                    fallback_result.unwrap()
                } else {
                    return Err(last_err);
                }
            }
            Err(e) => return Err(e),
        }
    };

    match provider_result {
        ProviderResponse::Complete(response) => {
            let latency_ms = start.elapsed().as_millis() as u32;
            let usage = response.usage.clone().unwrap_or_default();
            let completion_hash = hash_completion(&response);

            // Cache store
            if let Some(ref cache) = state.response_cache
                && let Some(cache_key) = cache_key.clone()
            {
                cache.insert(cache_key, response.clone()).await;
            }

            // Track if failover occurred
            let provider_attempted = if provider_name != primary_provider_name {
                Some(format!("{} -> {}", primary_provider_name, provider_name))
            } else {
                None
            };

            // Queue inference event
            let event_ctx = EventContext {
                trace_id: trace_id.clone(),
                span_id: span_id.clone(),
                parent_span_id: parent_span_id.clone(),
                agent_framework: agent_framework.clone(),
                tool_calls_json: tool_calls_json.clone(),
                ttft_ms: None, // non-streaming: no TTFT
                session_id: session_id.clone(),
                provider_attempted,
            };
            let event = build_event(
                &provider_name,
                &response.model,
                EventStatus::Success,
                &usage,
                latency_ms,
                &prompt_hash,
                &completion_hash,
                task_type,
                routing_decision_str,
                auth_ctx.as_ref(),
                variant_name,
                episode_id,
                request.user.clone(),
                &event_ctx,
            );

            // Post-request: record tokens + spend
            if let Some(ref ctx) = auth_ctx {
                state
                    .rate_limiter
                    .record_tokens(&ctx.key_hash, usage.total_tokens)
                    .await;
                state
                    .budget_tracker
                    .record_spend(&ctx.key_hash, event.estimated_cost_usd);
            }

            let event_id = event.id;
            let event_model = event.model.clone();
            let event_cost = event.estimated_cost_usd;
            let event_latency = latency_ms;
            let event_tokens = usage.total_tokens;
            #[cfg(feature = "otel")]
            crate::observability::otel::record_inference_span(&event);
            let _ = state.event_tx.send(event).await;

            // Record metrics
            if let Some(ref m) = state.metrics {
                m.record_request(&request_model, event_latency as u64, false);
                m.record_tokens(event_tokens as u64);
                m.record_cost(event_cost);
            }

            // Increment session cost (stored as micro-dollars for lock-free atomic operations)
            state.session_cost_usd.fetch_add(
                (event_cost * 1_000_000.0) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );

            // MCP tracing: extract MCP tool calls from tool_calls_json
            if let Some(ref mcp_tx) = state.mcp_tx {
                emit_mcp_calls(
                    mcp_tx,
                    tool_calls_json.as_deref(),
                    trace_id.as_deref(),
                    span_id.as_deref(),
                    parent_span_id.as_deref(),
                    event_id,
                    &event_model,
                    event_cost,
                )
                .await;
            }

            // Benchmark sampling hook (non-streaming)
            if let Some(ref benchmark_tx) = state.benchmark_tx
                && should_sample(state.config.benchmark.sample_rate)
            {
                let bench_req = crate::benchmark::BenchmarkRequest {
                    inference_id: uuid::Uuid::new_v4(),
                    request: request.clone(),
                    original_model: response.model.clone(),
                    original_completion: extract_completion_text(&response),
                    original_cost: compute_cost(&response.model, &usage),
                    original_latency_ms: latency_ms,
                    task_type,
                    prompt_hash: prompt_hash.clone(),
                };
                let _ = benchmark_tx.try_send(bench_req);
            }

            // JSON schema validation (non-streaming)
            validate_response_schema(&request, &response)?;

            tracing::info!(
                model = %response.model,
                input_tokens = usage.prompt_tokens,
                output_tokens = usage.completion_tokens,
                latency_ms,
                "completed"
            );

            let mut resp = Json(response).into_response();
            resp.headers_mut()
                .insert("x-cache", "MISS".parse().unwrap());
            if let Some(ref ctx) = auth_ctx {
                let rl_headers = build_rate_limit_headers(ctx, &state);
                resp.headers_mut().extend(rl_headers);
            }
            Ok(resp)
        }
        ProviderResponse::Stream(stream) => {
            let (relay, result_rx) = StreamRelay::start(stream);
            let event_tx = state.event_tx.clone();
            let provider_name_owned = provider_name.clone();
            let prompt_hash_owned = prompt_hash.clone();
            let rate_limiter = state.rate_limiter.clone();
            let budget_tracker = state.budget_tracker.clone();
            let auth_ctx_owned = auth_ctx.clone();
            let cache_clone = state.response_cache.clone();
            let request_clone = request.clone();
            let benchmark_tx_clone = state.benchmark_tx.clone();
            let benchmark_sample_rate = state.config.benchmark.sample_rate;
            let mcp_tx_clone = state.mcp_tx.clone();
            let trace_id_owned = trace_id.clone();
            let span_id_owned = span_id.clone();
            let parent_span_id_owned = parent_span_id.clone();
            let agent_framework_owned = agent_framework.clone();
            let tool_calls_json_owned = tool_calls_json.clone();
            let session_id_owned = session_id.clone();
            let session_cost_usd = state.session_cost_usd.clone();

            // Spawn a task to capture the final result after stream completes
            tokio::spawn(async move {
                if let Ok(result) = result_rx.await {
                    let latency_ms = start.elapsed().as_millis() as u32;
                    let completion_hash = hash_string(&result.completion_text);
                    let model = if result.model.is_empty() {
                        request_model.clone()
                    } else {
                        result.model.clone()
                    };

                    // Cache streamed response (before consuming result fields)
                    let reconstructed = reconstruct_response(&result, &model);

                    // JSON schema validation (streaming — post-hoc, log warning only)
                    if let Err(e) = validate_response_schema(&request_clone, &reconstructed) {
                        tracing::warn!(
                            error = %e,
                            model = %model,
                            "streaming response failed schema validation"
                        );
                    }

                    if let Some(ref cache) = cache_clone
                        && !no_cache
                    {
                        let cache_key = ResponseCache::cache_key(&request_clone);
                        cache.insert(cache_key, reconstructed).await;
                    }

                    let usage = result.usage.unwrap_or_default();

                    // Clone for MCP extraction before moving into EventContext
                    let mcp_trace_id = trace_id_owned.clone();
                    let mcp_span_id = span_id_owned.clone();
                    let mcp_parent_span_id = parent_span_id_owned.clone();
                    let mcp_tool_calls_json = tool_calls_json_owned.clone();

                    let event_ctx = EventContext {
                        trace_id: trace_id_owned,
                        span_id: span_id_owned,
                        parent_span_id: parent_span_id_owned,
                        agent_framework: agent_framework_owned,
                        tool_calls_json: tool_calls_json_owned,
                        ttft_ms: result.ttft_ms,
                        session_id: session_id_owned,
                        provider_attempted: None,
                    };
                    let event = build_event(
                        &provider_name_owned,
                        &model,
                        EventStatus::Success,
                        &usage,
                        latency_ms,
                        &prompt_hash_owned,
                        &completion_hash,
                        task_type,
                        routing_decision_str,
                        auth_ctx_owned.as_ref(),
                        variant_name,
                        episode_id,
                        request_clone.user.clone(),
                        &event_ctx,
                    );

                    // Post-stream: record tokens + spend
                    if let Some(ref ctx) = auth_ctx_owned {
                        rate_limiter
                            .record_tokens(&ctx.key_hash, usage.total_tokens)
                            .await;
                        budget_tracker.record_spend(&ctx.key_hash, event.estimated_cost_usd);
                    }

                    let event_id = event.id;
                    let event_model_str = event.model.clone();
                    let event_cost = event.estimated_cost_usd;
                    #[cfg(feature = "otel")]
                    crate::observability::otel::record_inference_span(&event);
                    let _ = event_tx.send(event).await;

                    // Increment session cost (stored as micro-dollars for lock-free atomic operations)
                    session_cost_usd.fetch_add(
                        (event_cost * 1_000_000.0) as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );

                    // MCP tracing: extract MCP tool calls from tool_calls_json
                    if let Some(ref mcp_tx) = mcp_tx_clone {
                        emit_mcp_calls(
                            mcp_tx,
                            mcp_tool_calls_json.as_deref(),
                            mcp_trace_id.as_deref(),
                            mcp_span_id.as_deref(),
                            mcp_parent_span_id.as_deref(),
                            event_id,
                            &event_model_str,
                            event_cost,
                        )
                        .await;
                    }

                    // Benchmark sampling hook (streaming)
                    if let Some(ref benchmark_tx) = benchmark_tx_clone
                        && should_sample(benchmark_sample_rate)
                    {
                        let bench_req = crate::benchmark::BenchmarkRequest {
                            inference_id: uuid::Uuid::new_v4(),
                            request: request_clone.clone(),
                            original_model: model.clone(),
                            original_completion: result.completion_text.clone(),
                            original_cost: compute_cost(&model, &usage),
                            original_latency_ms: latency_ms,
                            task_type,
                            prompt_hash: prompt_hash_owned.clone(),
                        };
                        let _ = benchmark_tx.try_send(bench_req);
                    }

                    tracing::info!(
                        model = %model,
                        input_tokens = usage.prompt_tokens,
                        output_tokens = usage.completion_tokens,
                        latency_ms,
                        ttft_ms = ?result.ttft_ms,
                        "stream completed"
                    );
                }
            });

            // Convert relay to SSE response.
            // Providers emit pre-framed SSE bytes ("data: {json}\n\n"). Strip the
            // "data: " prefix before passing to axum's Event::data(), which re-adds
            // it — otherwise clients receive "data: data: {...}".
            let sse_stream = futures::StreamExt::flat_map(relay, |item| {
                let events: Vec<std::result::Result<Event, std::convert::Infallible>> = match item {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).into_owned();
                        text.split("\n\n")
                            .filter_map(|block| {
                                block
                                    .lines()
                                    .find_map(|line| {
                                        line.strip_prefix("data: ").map(str::to_string)
                                    })
                                    .map(|payload| Ok(Event::default().data(payload)))
                            })
                            .collect()
                    }
                    Err(_) => vec![],
                };
                futures::stream::iter(events)
            });

            let mut sse_resp = Sse::new(sse_stream)
                .keep_alive(KeepAlive::default())
                .into_response();
            if let Some(ref ctx) = auth_ctx {
                let rl_headers = build_rate_limit_headers(ctx, &state);
                sse_resp.headers_mut().extend(rl_headers);
            }
            Ok(sse_resp)
        }
    }
}

/// POST /v1/embeddings
pub async fn embeddings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<EmbeddingRequest>,
) -> Result<Response> {
    let start = Instant::now();
    let (provider_name, model_id) = resolve_model(&state.config, &request.model)?;
    let provider = state.providers.get(&provider_name)?;

    let response = provider.embed(&request, &model_id).await?;
    let latency_ms = start.elapsed().as_millis() as u32;

    tracing::info!(
        model = %response.model,
        tokens = response.usage.total_tokens,
        latency_ms,
        "embedding completed"
    );

    Ok(Json(response).into_response())
}

// ---------------------------------------------------------------------------
// Classifier input builder
// ---------------------------------------------------------------------------

fn build_classifier_input(request: &ChatCompletionRequest) -> ClassifierInput {
    let has_tools = request.tools.as_ref().map_or(false, |t| !t.is_empty());
    let tool_count = request.tools.as_ref().map_or(0, |t| t.len());

    // Check for JSON schema in response_format
    let has_json_schema = request
        .response_format
        .as_ref()
        .and_then(|rf| rf.get("type"))
        .and_then(|t| t.as_str())
        .map_or(false, |t| t == "json_schema" || t == "json_object");

    // Extract system prompt text
    let system_prompt_text: Option<String> = request
        .messages
        .iter()
        .find(|m| m.role == "system")
        .and_then(|m| m.content.as_ref())
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());

    let has_code_fence_in_system = system_prompt_text
        .as_ref()
        .map_or(false, |s| s.contains("```"));

    // Get last user message
    let last_user_message = request
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.as_ref())
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    // Check if any assistant message has tool_calls
    let has_tool_calls = request
        .messages
        .iter()
        .any(|m| m.tool_calls.as_ref().map_or(false, |tc| !tc.is_empty()));

    // Detect output format hint from response_format
    let output_format_hint = request
        .response_format
        .as_ref()
        .and_then(|rf| rf.get("type"))
        .and_then(|t| t.as_str())
        .and_then(|t| match t {
            "json_schema" | "json_object" => Some(OutputFormatHint::Json),
            _ => None,
        });

    let system_prompt_hash = system_prompt_text.as_ref().map(|s| hash_string(s));

    // Detect FIM: presence of a `suffix` key in the extra pass-through fields.
    let has_fim = request.extra.contains_key("suffix");

    ClassifierInput {
        system_prompt_hash,
        has_tools,
        tool_count,
        has_json_schema,
        has_code_fence_in_system,
        prompt_tokens: 0, // not known pre-request
        completion_tokens: 0,
        token_ratio: 0.0,
        model: request.model.clone(),
        has_tool_calls,
        output_format_hint,
        last_user_message,
        system_prompt_text,
        has_fim,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a user-facing model name to (provider_name, provider_model_id).
pub(crate) fn resolve_model(config: &Config, model: &str) -> Result<(String, String)> {
    // Check configured model aliases first
    if let Some(model_config) = config.models.get(model) {
        return Ok((model_config.provider.clone(), model_config.model.clone()));
    }

    // Check semantic aliases (fast, smart, cheap, etc.)
    if let Some(resolved) = models::resolve_alias(model) {
        if let Some(info) = models::lookup_model(resolved) {
            return Ok((info.provider.to_string(), info.model_id.to_string()));
        }
    }

    // Check the static catalog
    if let Some(info) = models::lookup_model(model) {
        return Ok((info.provider.to_string(), info.model_id.to_string()));
    }

    // Fall back to provider inference from model name
    let provider = models::infer_provider(model);
    Ok((provider.to_string(), model.to_string()))
}

/// Resolve a model to its primary provider + any fallback providers.
pub(crate) fn resolve_model_with_fallbacks(
    config: &Config,
    model: &str,
) -> Result<(String, String, Vec<(String, String)>)> {
    let (provider, model_id) = resolve_model(config, model)?;
    let fallbacks = config
        .models
        .get(model)
        .map(|mc| {
            mc.fallback_providers
                .iter()
                .map(|fp| (fp.provider.clone(), fp.model.clone()))
                .collect()
        })
        .unwrap_or_default();
    Ok((provider, model_id, fallbacks))
}

fn hash_messages(messages: &[crate::types::Message]) -> String {
    let mut hasher = Sha256::new();
    for msg in messages {
        hasher.update(msg.role.as_bytes());
        if let Some(content) = &msg.content {
            hasher.update(content.to_string().as_bytes());
        }
    }
    hex::encode(hasher.finalize())
}

fn hash_completion(response: &crate::types::ChatCompletionResponse) -> String {
    let text: String = response
        .choices
        .iter()
        .filter_map(|c| c.message.content.as_ref().and_then(|v| v.as_str()))
        .collect();
    hash_string(&text)
}

fn hash_string(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// Contextual data for building inference events (avoids too-many-arguments).
pub(crate) struct EventContext {
    pub(crate) trace_id: Option<String>,
    pub(crate) span_id: Option<String>,
    pub(crate) parent_span_id: Option<String>,
    pub(crate) agent_framework: Option<String>,
    pub(crate) tool_calls_json: Option<String>,
    pub(crate) ttft_ms: Option<u32>,
    pub(crate) session_id: Option<String>,
    pub(crate) provider_attempted: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_event(
    provider: &str,
    model: &str,
    status: EventStatus,
    usage: &Usage,
    latency_ms: u32,
    prompt_hash: &str,
    completion_hash: &str,
    task_type: Option<crate::types::TaskType>,
    routing_decision: Option<String>,
    auth_ctx: Option<&AuthContext>,
    variant_name: Option<String>,
    episode_id: Uuid,
    end_user_id: Option<String>,
    ctx: &EventContext,
) -> InferenceEvent {
    let cost = compute_cost(model, usage);
    InferenceEvent {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        provider: provider.to_string(),
        model: model.to_string(),
        status,
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        cache_creation_input_tokens: usage.cache_creation_input_tokens,
        estimated_cost_usd: cost,
        latency_ms,
        prompt_hash: prompt_hash.to_string(),
        completion_hash: completion_hash.to_string(),
        task_type,
        routing_decision,
        variant_name,
        virtual_key_hash: auth_ctx.map(|a| a.key_hash.clone()),
        team_id: auth_ctx.and_then(|a| a.team_id.clone()),
        end_user_id,
        episode_id: Some(episode_id),
        metadata: "{}".to_string(),
        trace_id: ctx.trace_id.clone(),
        span_id: ctx.span_id.clone(),
        parent_span_id: ctx.parent_span_id.clone(),
        agent_framework: ctx.agent_framework.clone(),
        tool_calls_json: ctx.tool_calls_json.clone(),
        ttft_ms: ctx.ttft_ms,
        session_id: ctx.session_id.clone(),
        provider_attempted: ctx.provider_attempted.clone(),
    }
}

/// Detect agent framework from User-Agent or other headers.
fn detect_agent_framework(headers: &HeaderMap) -> Option<String> {
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ua_lower = ua.to_lowercase();
    if ua_lower.contains("langchain") {
        Some("langchain".to_string())
    } else if ua_lower.contains("crewai") {
        Some("crewai".to_string())
    } else if ua_lower.contains("autogen") {
        Some("autogen".to_string())
    } else if ua_lower.contains("llamaindex") {
        Some("llamaindex".to_string())
    } else {
        None
    }
}

/// Extract tool calls from request messages as JSON string for observability.
fn extract_tool_calls_json(request: &ChatCompletionRequest) -> Option<String> {
    let tool_calls: Vec<&serde_json::Value> = request
        .messages
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flatten()
        .collect();
    if tool_calls.is_empty() {
        None
    } else {
        serde_json::to_string(&tool_calls).ok()
    }
}

fn should_sample(rate: f64) -> bool {
    let sample: f64 = rand::rng().random();
    sample < rate
}

fn extract_completion_text(response: &ChatCompletionResponse) -> String {
    response
        .choices
        .iter()
        .filter_map(|c| c.message.content.as_ref().and_then(|v| v.as_str()))
        .collect()
}

/// Prepend text to the system message, or insert a new system message at the start.
fn prepend_system_prompt(messages: &mut Vec<crate::types::Message>, prefix: &str) {
    if let Some(sys_msg) = messages.iter_mut().find(|m| m.role == "system") {
        let existing = sys_msg
            .content
            .as_ref()
            .and_then(|v| v.as_str())
            .unwrap_or("");
        sys_msg.content = Some(serde_json::Value::String(format!("{prefix}\n{existing}")));
    } else {
        messages.insert(
            0,
            crate::types::Message {
                role: "system".to_string(),
                content: Some(serde_json::Value::String(prefix.to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            },
        );
    }
}

/// Reconstruct a ChatCompletionResponse from a StreamResult, for caching.
fn reconstruct_response(
    result: &crate::proxy::streaming::StreamResult,
    model: &str,
) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: format!("chatcmpl-cache-{}", Uuid::new_v4()),
        object: "chat.completion".to_string(),
        created: Utc::now().timestamp(),
        model: model.to_string(),
        choices: vec![crate::types::Choice {
            index: 0,
            message: crate::types::Message {
                role: "assistant".to_string(),
                content: Some(serde_json::Value::String(result.completion_text.clone())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: result.usage.clone(),
        extra: serde_json::Map::new(),
    }
}

/// Validate a completion response against a JSON schema from response_format.
/// Returns Ok(()) if no schema is specified or validation passes.
fn validate_response_schema(
    request: &ChatCompletionRequest,
    response: &ChatCompletionResponse,
) -> Result<()> {
    let schema_value = match request.response_format.as_ref() {
        Some(rf) => {
            let rf_type = rf.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if rf_type != "json_schema" {
                return Ok(());
            }
            // Schema is at response_format.json_schema.schema
            rf.get("json_schema")
                .and_then(|js| js.get("schema"))
                .cloned()
        }
        None => return Ok(()),
    };

    let Some(schema_value) = schema_value else {
        return Ok(());
    };

    // Get the completion content to validate
    let content = response
        .choices
        .first()
        .and_then(|c| c.message.content.as_ref())
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if content.is_empty() {
        return Ok(());
    }

    // Parse completion as JSON
    let instance: serde_json::Value = serde_json::from_str(content).map_err(|e| {
        PrismError::SchemaValidationFailed(format!("response is not valid JSON: {e}"))
    })?;

    // Validate against schema
    let validator = jsonschema::validator_for(&schema_value).map_err(|e| {
        PrismError::Internal(format!("invalid JSON schema in response_format: {e}"))
    })?;

    let errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| format!("{} at {}", e, e.instance_path))
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(PrismError::SchemaValidationFailed(errors.join("; ")))
    }
}

/// Build `X-RateLimit-*` headers from the auth context and current rate limiter state.
fn build_rate_limit_headers(ctx: &AuthContext, state: &Arc<AppState>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(rpm_limit) = ctx.rpm_limit {
        let current = state.rate_limiter.current_rpm(&ctx.key_hash);
        let remaining = (rpm_limit as usize).saturating_sub(current);
        if let (Ok(limit_val), Ok(rem_val)) =
            (rpm_limit.to_string().parse(), remaining.to_string().parse())
        {
            headers.insert("x-ratelimit-limit-requests", limit_val);
            headers.insert("x-ratelimit-remaining-requests", rem_val);
            headers.insert("x-ratelimit-reset-requests", "60".parse().unwrap());
        }
    }
    if let Some(tpm_limit) = ctx.tpm_limit {
        let current = state.rate_limiter.current_tpm(&ctx.key_hash);
        let remaining = (tpm_limit as u32).saturating_sub(current);
        if let (Ok(limit_val), Ok(rem_val)) =
            (tpm_limit.to_string().parse(), remaining.to_string().parse())
        {
            headers.insert("x-ratelimit-limit-tokens", limit_val);
            headers.insert("x-ratelimit-remaining-tokens", rem_val);
            headers.insert("x-ratelimit-reset-tokens", "60".parse().unwrap());
        }
    }
    headers
}

/// Emit MCP tool call events extracted from tool_calls_json.
async fn emit_mcp_calls(
    mcp_tx: &tokio::sync::mpsc::Sender<McpCall>,
    tool_calls_json: Option<&str>,
    trace_id: Option<&str>,
    span_id: Option<&str>,
    parent_span_id: Option<&str>,
    inference_id: Uuid,
    model: &str,
    estimated_cost: f64,
) {
    let Some(json) = tool_calls_json else {
        return;
    };
    let calls = extract_mcp_calls(json);
    if calls.is_empty() {
        return;
    }

    let per_call_cost = estimated_cost / calls.len() as f64;
    let trace = trace_id.unwrap_or("").to_string();

    for call in calls {
        let mcp_call = McpCall {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            trace_id: trace.clone(),
            span_id: span_id.map(|s| s.to_string()),
            parent_span_id: parent_span_id.map(|s| s.to_string()),
            server: call.server,
            method: call.method,
            tool_name: call.tool_name,
            args_hash: call.args_hash,
            inference_id,
            model: model.to_string(),
            estimated_cost: per_call_cost,
        };
        let _ = mcp_tx.send(mcp_call).await;
    }
}

/// Shared application state passed to all handlers.
pub struct AppState {
    pub config: Config,
    pub providers: Arc<ProviderRegistry>,
    pub event_tx: tokio::sync::mpsc::Sender<InferenceEvent>,
    pub http_client: reqwest::Client,
    pub fitness_cache: FitnessCache,
    pub routing_policy: RoutingPolicy,
    pub key_service: Option<Arc<KeyService>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub budget_tracker: Arc<BudgetTracker>,
    pub experiment_engine: Option<Arc<ExperimentEngine>>,
    pub response_cache: Option<Arc<ResponseCache>>,
    pub feedback_tx: Option<tokio::sync::mpsc::Sender<FeedbackEvent>>,
    pub benchmark_tx: Option<tokio::sync::mpsc::Sender<crate::benchmark::BenchmarkRequest>>,
    pub mcp_tx: Option<tokio::sync::mpsc::Sender<crate::mcp::types::McpCall>>,
    pub hot_config: Option<Arc<ArcSwap<Config>>>,
    pub hot_routing_policy: Option<Arc<ArcSwap<RoutingPolicy>>>,
    pub prompt_store: Option<Arc<crate::prompts::store::PromptStore>>,
    pub session_tracker: Arc<tokio::sync::Mutex<crate::routing::session::SessionTracker>>,
    pub callback_registry: Option<Arc<crate::observability::callbacks::CallbackRegistry>>,
    pub interop_bridge: Option<Arc<crate::interop::bridge::DiscoveryBridge>>,
    pub interop_metering: Option<Arc<crate::interop::metering::MeteringStore>>,
    pub metrics: Option<Arc<crate::observability::metrics::MetricsCollector>>,
    pub session_cost_usd: Arc<std::sync::atomic::AtomicU64>,
    // Phase 4 additions
    pub health_tracker: Option<Arc<ProviderHealthTracker>>,
    pub audit_service: Option<Arc<AuditService>>,
    pub alias_cache: Option<Arc<AliasCache>>,
    pub alias_repo: Option<Arc<AliasRepository>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_sample_statistical() {
        // With rate=1.0, always sample
        let mut sampled = 0;
        for _ in 0..100 {
            if should_sample(1.0) {
                sampled += 1;
            }
        }
        assert_eq!(sampled, 100);

        // With rate=0.0, never sample
        sampled = 0;
        for _ in 0..100 {
            if should_sample(0.0) {
                sampled += 1;
            }
        }
        assert_eq!(sampled, 0);

        // With rate=0.5, should be roughly 50% (allow wide margin for randomness)
        sampled = 0;
        for _ in 0..1000 {
            if should_sample(0.5) {
                sampled += 1;
            }
        }
        assert!(
            sampled > 300 && sampled < 700,
            "sampled {sampled}/1000 at rate=0.5"
        );
    }

    #[test]
    fn extract_completion_text_concatenates_choices() {
        let response = ChatCompletionResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test-model".to_string(),
            choices: vec![
                crate::types::Choice {
                    index: 0,
                    message: crate::types::Message {
                        role: "assistant".to_string(),
                        content: Some(serde_json::Value::String("Hello ".to_string())),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                        extra: serde_json::Map::new(),
                    },
                    finish_reason: Some("stop".to_string()),
                },
                crate::types::Choice {
                    index: 1,
                    message: crate::types::Message {
                        role: "assistant".to_string(),
                        content: Some(serde_json::Value::String("World".to_string())),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                        extra: serde_json::Map::new(),
                    },
                    finish_reason: Some("stop".to_string()),
                },
            ],
            usage: None,
            extra: serde_json::Map::new(),
        };

        let text = extract_completion_text(&response);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn resolve_model_with_fallbacks_returns_primary_and_fallbacks() {
        use crate::config::{FallbackProvider, ModelConfig};

        let mut config: Config = figment::Figment::new().extract().unwrap();
        config.models.insert(
            "claude-sonnet".to_string(),
            ModelConfig {
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4-20250514".to_string(),
                tier: None,
                max_tokens: None,
                context_window: None,
                fallback_providers: vec![
                    FallbackProvider {
                        provider: "bedrock".to_string(),
                        model: "anthropic.claude-sonnet-4-20250514-v1:0".to_string(),
                    },
                    FallbackProvider {
                        provider: "openai_compat".to_string(),
                        model: "claude-sonnet-4".to_string(),
                    },
                ],
                supports_tools: true,
            },
        );

        let (provider, model_id, fallbacks) =
            resolve_model_with_fallbacks(&config, "claude-sonnet").unwrap();
        assert_eq!(provider, "anthropic");
        assert_eq!(model_id, "claude-sonnet-4-20250514");
        assert_eq!(fallbacks.len(), 2);
        assert_eq!(fallbacks[0].0, "bedrock");
        assert_eq!(fallbacks[0].1, "anthropic.claude-sonnet-4-20250514-v1:0");
        assert_eq!(fallbacks[1].0, "openai_compat");
        assert_eq!(fallbacks[1].1, "claude-sonnet-4");
    }

    #[test]
    fn resolve_model_with_fallbacks_no_fallbacks_returns_empty() {
        use crate::config::ModelConfig;

        let mut config: Config = figment::Figment::new().extract().unwrap();
        config.models.insert(
            "gpt-4o".to_string(),
            ModelConfig {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                tier: None,
                max_tokens: None,
                context_window: None,
                fallback_providers: vec![],
                supports_tools: true,
            },
        );

        let (provider, model_id, fallbacks) =
            resolve_model_with_fallbacks(&config, "gpt-4o").unwrap();
        assert_eq!(provider, "openai");
        assert_eq!(model_id, "gpt-4o");
        assert!(fallbacks.is_empty());
    }

    #[test]
    fn resolve_model_with_fallbacks_unconfigured_model_no_fallbacks() {
        let config: Config = figment::Figment::new().extract().unwrap();

        // Model not in config.models, falls through to catalog/inference
        let (_, _, fallbacks) = resolve_model_with_fallbacks(&config, "gpt-4o-mini").unwrap();
        assert!(fallbacks.is_empty());
    }
}
