use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::middleware::from_fn;
#[cfg(feature = "full")]
use axum::routing::{delete, patch, put};
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::api;
use crate::api::openapi::ApiDoc;
use crate::experiment::feedback;
#[cfg(feature = "full")]
use crate::keys::MasterKey;
use crate::proxy::batch;
use crate::proxy::handler::{self, AppState};
use crate::server::middleware;
#[cfg(feature = "full")]
use crate::waste;

/// Build the axum router with all routes and middleware.
pub fn build(state: Arc<AppState>) -> Router {
    // --- Proxy routes (with optional key service extension) ---
    let proxy_routes = Router::new()
        .route("/v1/chat/completions", post(handler::chat_completions))
        .route(
            "/v1/completions",
            post(crate::proxy::completions_handler::text_completions),
        )
        .route("/v1/embeddings", post(handler::embeddings))
        .route(
            "/v1/batch/chat/completions",
            post(batch::batch_chat_completions),
        )
        .route(
            "/v1/messages",
            post(crate::proxy::anthropic_handler::anthropic_messages),
        )
        .route(
            "/v1/edit_predictions",
            post(crate::proxy::predict_edits::predict_edits),
        )
        .route("/v1/models", get(api::models::list_models))
        .route("/api/v1/feedback", post(feedback::submit_feedback));

    // Inject key service into request extensions
    let key_service = state.key_service.clone();

    let proxy_routes = proxy_routes.layer(from_fn(
        move |mut req: Request, next: axum::middleware::Next| {
            let ks = key_service.clone();
            async move {
                if let Some(ks) = ks {
                    req.extensions_mut().insert(ks);
                }
                next.run(req).await
            }
        },
    ));

    // --- Management routes (master key auth) ---
    let management_routes = build_management_routes(&state);

    // --- Health routes (no auth) ---
    let health_routes = Router::new()
        .route("/health", get(api::health::health_with_state))
        .route("/health/live", get(api::health::liveness))
        .route("/health/ready", get(api::health::readiness))
        .route("/health/providers", get(api::health::provider_health))
        .route("/metrics", get(api::metrics::metrics));

    // --- OpenAPI docs (stateless, merged before state is applied) ---
    let swagger_routes: Router<Arc<AppState>> =
        Router::new().merge(SwaggerUi::new("/docs").url("/openapi.json", ApiDoc::openapi()));

    // --- Dashboard static files ---
    let mut app = Router::new()
        .merge(proxy_routes)
        .merge(management_routes)
        .merge(health_routes)
        .merge(swagger_routes);

    if state.config.dashboard.enabled {
        let dist_path = &state.config.dashboard.dist_path;
        if std::path::Path::new(dist_path).exists() {
            tracing::info!(path = dist_path, "serving dashboard");
            app = app.nest_service("/dashboard", tower_http::services::ServeDir::new(dist_path));
        } else {
            tracing::warn!(path = dist_path, "dashboard dist path not found, skipping");
        }
    }

    app.layer(middleware::cors_layer(&state.config.cors))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Build management routes — key CRUD endpoints authenticated with master key.
/// Only available in full builds; returns an empty router in embedded/lean builds.
#[cfg(not(feature = "full"))]
fn build_management_routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    let _ = state;
    Router::new()
}

#[cfg(feature = "full")]
fn build_management_routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    if !state.config.keys.enabled {
        return Router::new();
    }

    let master_key = state.config.keys.master_key.clone().unwrap_or_default();

    if master_key.is_empty() {
        tracing::warn!("keys.enabled but no master_key configured — management API disabled");
        return Router::new();
    }

    let mk = MasterKey(master_key);

    Router::new()
        .route("/api/v1/keys", post(api::keys::create_key))
        .route("/api/v1/keys", get(api::keys::list_keys))
        .route("/api/v1/keys/{id}", delete(api::keys::revoke_key))
        .route("/api/v1/keys/{id}", patch(api::keys::update_key))
        .route("/api/v1/keys/{id}/usage", get(api::keys::key_usage))
        .route("/api/v1/waste-report", get(waste::handler::waste_report))
        .route("/api/v1/stats/summary", get(api::stats::summary))
        .route("/api/v1/stats/timeseries", get(api::stats::timeseries))
        .route("/api/v1/stats/top-traces", get(api::stats::top_traces))
        .route("/api/v1/stats/waste-score", get(api::stats::waste_score))
        .route("/api/v1/stats/task-types", get(api::stats::task_type_stats))
        .route("/api/v1/stats/agents", get(api::stats::agent_metrics))
        .route("/v1/costs", get(api::costs::thread_costs))
        .route("/api/v1/alerts/rules", get(api::alerts::list_rules))
        .route("/api/v1/mcp/trace", get(api::mcp::mcp_trace))
        .route("/api/v1/routing/dry-run", post(api::routing::dry_run))
        .route("/api/v1/routing/validate", post(api::routing::validate))
        .route("/api/v1/routing/policy", get(api::routing::get_policy))
        .route(
            "/api/v1/config/reload",
            post(api::config_reload::reload_config),
        )
        .route("/api/v1/config", get(api::config_reload::get_config))
        .route(
            "/api/v1/compliance/export",
            get(crate::compliance::export::export_compliance),
        )
        .route("/api/v1/prompts", post(api::prompts::create_prompt))
        .route("/api/v1/prompts", get(api::prompts::list_prompts))
        .route("/api/v1/prompts/{name}", get(api::prompts::get_prompt))
        .route("/api/v1/workflows/execute", post(api::workflows::execute))
        .route(
            "/api/v1/finetuning/export",
            post(crate::finetuning::export::export_training_data),
        )
        // Key rotation
        .route("/api/v1/keys/{id}/rotate", post(api::keys::rotate_key))
        // Audit log
        .route("/api/v1/audit", get(api::audit::list_audit_events))
        // Model aliases
        .route("/api/v1/aliases", get(api::aliases::list_aliases))
        .route("/api/v1/aliases", post(api::aliases::create_alias))
        .route("/api/v1/aliases/{name}", put(api::aliases::update_alias))
        .route("/api/v1/aliases/{name}", delete(api::aliases::delete_alias))
        // Prompt version history & rollback
        .route(
            "/api/v1/prompts/{name}/versions",
            get(api::prompts::get_versions),
        )
        .route(
            "/api/v1/prompts/{name}/rollback/{version}",
            post(api::prompts::rollback_prompt),
        )
        // Billing
        .route(
            "/api/v1/billing/reconcile",
            post(api::billing::reconcile_billing),
        )
        .route("/api/v1/billing/usage", get(api::billing::get_usage))
        // Interop
        .route("/api/v1/interop/invoke", post(api::interop::invoke))
        .route("/api/v1/interop/register", post(api::interop::register))
        .route("/api/v1/interop/discover", get(api::interop::discover))
        .route(
            "/api/v1/interop/metering",
            get(api::interop::metering_summary),
        )
        // Budget hierarchy
        .route(
            "/api/v1/budgets/hierarchy",
            get(api::budgets::get_hierarchy),
        )
        .route("/api/v1/budgets/nodes", post(api::budgets::create_node))
        .route("/api/v1/budgets/nodes/{id}", put(api::budgets::update_node))
        .route(
            "/api/v1/budgets/nodes/{id}",
            delete(api::budgets::delete_node),
        )
        .layer(from_fn(
            move |mut req: Request, next: axum::middleware::Next| {
                let mk = mk.clone();
                async move {
                    req.extensions_mut().insert(mk);
                    next.run(req).await
                }
            },
        ))
}
