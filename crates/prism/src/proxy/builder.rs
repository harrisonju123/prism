use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::Mutex;

use crate::benchmark::BenchmarkRequest;
use crate::cache::ResponseCache;
use crate::config::Config;
use crate::experiment::engine::ExperimentEngine;
use crate::experiment::feedback::FeedbackEvent;
use crate::interop::bridge::DiscoveryBridge;
use crate::interop::metering::MeteringStore;
use crate::keys::KeyService;
use crate::keys::audit::AuditService;
use crate::keys::budget::BudgetTracker;
use crate::keys::rate_limit::RateLimiter;
use crate::mcp::types::McpCall;
use crate::models::alias::{AliasCache, AliasRepository};
use crate::observability::callbacks::CallbackRegistry;
use crate::observability::metrics::MetricsCollector;
use crate::prompts::store::PromptStore;
use crate::providers::health::ProviderHealthTracker;
use crate::providers::{CircuitBreakerMap, ProviderRegistry, new_circuit_breaker_map};
use crate::proxy::handler::AppState;
use crate::routing::FitnessCache;
use crate::routing::session::SessionTracker;
use crate::routing::types::RoutingPolicy;
use crate::types::InferenceEvent;

#[derive(Debug, thiserror::Error)]
pub enum AppStateBuildError {
    #[error("providers must be set via with_providers()")]
    MissingProviders,
    #[error("event_tx must be set via with_event_tx()")]
    MissingEventTx,
}

pub struct AppStateBuilder {
    config: Config,
    // required
    providers: Option<Arc<ProviderRegistry>>,
    event_tx: Option<tokio::sync::mpsc::Sender<InferenceEvent>>,
    // config-driven defaults
    fitness_cache: Option<FitnessCache>,
    routing_policy: Option<RoutingPolicy>,
    rate_limiter: Option<Arc<RateLimiter>>,
    budget_tracker: Option<Arc<BudgetTracker>>,
    session_tracker: Option<Arc<Mutex<SessionTracker>>>,
    // optional features
    key_service: Option<Arc<KeyService>>,
    experiment_engine: Option<Arc<ExperimentEngine>>,
    response_cache: Option<Arc<ResponseCache>>,
    feedback_tx: Option<tokio::sync::mpsc::Sender<FeedbackEvent>>,
    benchmark_tx: Option<tokio::sync::mpsc::Sender<BenchmarkRequest>>,
    mcp_tx: Option<tokio::sync::mpsc::Sender<McpCall>>,
    hot_config: Option<Arc<ArcSwap<Config>>>,
    hot_routing_policy: Option<Arc<ArcSwap<RoutingPolicy>>>,
    prompt_store: Option<Arc<PromptStore>>,
    callback_registry: Option<Arc<CallbackRegistry>>,
    interop_bridge: Option<Arc<DiscoveryBridge>>,
    interop_metering: Option<Arc<MeteringStore>>,
    metrics: Option<Arc<MetricsCollector>>,
    session_cost_usd: Option<Arc<std::sync::atomic::AtomicU64>>,
    // Phase 4
    health_tracker: Option<Arc<ProviderHealthTracker>>,
    audit_service: Option<Arc<AuditService>>,
    alias_cache: Option<Arc<AliasCache>>,
    alias_repo: Option<Arc<AliasRepository>>,
    circuit_breakers: Option<CircuitBreakerMap>,
    session_spend: Option<Arc<dashmap::DashMap<uuid::Uuid, f64>>>,
    uh_store: Option<Arc<uglyhat::store::sqlite::SqliteStore>>,
    uh_workspace_id: Option<uuid::Uuid>,
    pg_pool: Option<sqlx::PgPool>,
}

impl AppStateBuilder {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            providers: None,
            event_tx: None,
            fitness_cache: None,
            routing_policy: None,
            rate_limiter: None,
            budget_tracker: None,
            session_tracker: None,
            key_service: None,
            experiment_engine: None,
            response_cache: None,
            feedback_tx: None,
            benchmark_tx: None,
            mcp_tx: None,
            hot_config: None,
            hot_routing_policy: None,
            prompt_store: None,
            callback_registry: None,
            interop_bridge: None,
            interop_metering: None,
            metrics: None,
            session_cost_usd: None,
            health_tracker: None,
            audit_service: None,
            alias_cache: None,
            alias_repo: None,
            circuit_breakers: None,
            session_spend: None,
            uh_store: None,
            uh_workspace_id: None,
            pg_pool: None,
        }
    }

    // --- Required setters ---

    pub fn with_providers(mut self, providers: Arc<ProviderRegistry>) -> Self {
        self.providers = Some(providers);
        self
    }

    pub fn with_event_tx(mut self, event_tx: tokio::sync::mpsc::Sender<InferenceEvent>) -> Self {
        self.event_tx = Some(event_tx);
        self
    }

    // --- Config-driven overridable setters ---

    pub fn with_fitness_cache(mut self, fitness_cache: FitnessCache) -> Self {
        self.fitness_cache = Some(fitness_cache);
        self
    }

    pub fn with_routing_policy(mut self, routing_policy: RoutingPolicy) -> Self {
        self.routing_policy = Some(routing_policy);
        self
    }

    pub fn with_rate_limiter(mut self, rate_limiter: Arc<RateLimiter>) -> Self {
        self.rate_limiter = Some(rate_limiter);
        self
    }

    pub fn with_budget_tracker(mut self, budget_tracker: Arc<BudgetTracker>) -> Self {
        self.budget_tracker = Some(budget_tracker);
        self
    }

    pub fn with_session_tracker(mut self, session_tracker: Arc<Mutex<SessionTracker>>) -> Self {
        self.session_tracker = Some(session_tracker);
        self
    }

    // --- Optional feature setters (bare value variants) ---

    pub fn with_key_service(mut self, key_service: Arc<KeyService>) -> Self {
        self.key_service = Some(key_service);
        self
    }

    pub fn with_key_service_opt(mut self, key_service: Option<Arc<KeyService>>) -> Self {
        self.key_service = key_service;
        self
    }

    pub fn with_experiment_engine(mut self, experiment_engine: Arc<ExperimentEngine>) -> Self {
        self.experiment_engine = Some(experiment_engine);
        self
    }

    pub fn with_experiment_engine_opt(
        mut self,
        experiment_engine: Option<Arc<ExperimentEngine>>,
    ) -> Self {
        self.experiment_engine = experiment_engine;
        self
    }

    pub fn with_response_cache(mut self, response_cache: Arc<ResponseCache>) -> Self {
        self.response_cache = Some(response_cache);
        self
    }

    pub fn with_response_cache_opt(mut self, response_cache: Option<Arc<ResponseCache>>) -> Self {
        self.response_cache = response_cache;
        self
    }

    pub fn with_feedback_tx(
        mut self,
        feedback_tx: tokio::sync::mpsc::Sender<FeedbackEvent>,
    ) -> Self {
        self.feedback_tx = Some(feedback_tx);
        self
    }

    pub fn with_feedback_tx_opt(
        mut self,
        feedback_tx: Option<tokio::sync::mpsc::Sender<FeedbackEvent>>,
    ) -> Self {
        self.feedback_tx = feedback_tx;
        self
    }

    pub fn with_benchmark_tx(
        mut self,
        benchmark_tx: tokio::sync::mpsc::Sender<BenchmarkRequest>,
    ) -> Self {
        self.benchmark_tx = Some(benchmark_tx);
        self
    }

    pub fn with_benchmark_tx_opt(
        mut self,
        benchmark_tx: Option<tokio::sync::mpsc::Sender<BenchmarkRequest>>,
    ) -> Self {
        self.benchmark_tx = benchmark_tx;
        self
    }

    pub fn with_mcp_tx(mut self, mcp_tx: tokio::sync::mpsc::Sender<McpCall>) -> Self {
        self.mcp_tx = Some(mcp_tx);
        self
    }

    pub fn with_mcp_tx_opt(mut self, mcp_tx: Option<tokio::sync::mpsc::Sender<McpCall>>) -> Self {
        self.mcp_tx = mcp_tx;
        self
    }

    pub fn with_hot_config(mut self, hot_config: Arc<ArcSwap<Config>>) -> Self {
        self.hot_config = Some(hot_config);
        self
    }

    pub fn with_hot_config_opt(mut self, hot_config: Option<Arc<ArcSwap<Config>>>) -> Self {
        self.hot_config = hot_config;
        self
    }

    pub fn with_hot_routing_policy(
        mut self,
        hot_routing_policy: Arc<ArcSwap<RoutingPolicy>>,
    ) -> Self {
        self.hot_routing_policy = Some(hot_routing_policy);
        self
    }

    pub fn with_hot_routing_policy_opt(
        mut self,
        hot_routing_policy: Option<Arc<ArcSwap<RoutingPolicy>>>,
    ) -> Self {
        self.hot_routing_policy = hot_routing_policy;
        self
    }

    pub fn with_prompt_store(mut self, prompt_store: Arc<PromptStore>) -> Self {
        self.prompt_store = Some(prompt_store);
        self
    }

    pub fn with_prompt_store_opt(mut self, prompt_store: Option<Arc<PromptStore>>) -> Self {
        self.prompt_store = prompt_store;
        self
    }

    pub fn with_callback_registry(mut self, callback_registry: Arc<CallbackRegistry>) -> Self {
        self.callback_registry = Some(callback_registry);
        self
    }

    pub fn with_callback_registry_opt(
        mut self,
        callback_registry: Option<Arc<CallbackRegistry>>,
    ) -> Self {
        self.callback_registry = callback_registry;
        self
    }

    pub fn with_interop_bridge(mut self, interop_bridge: Arc<DiscoveryBridge>) -> Self {
        self.interop_bridge = Some(interop_bridge);
        self
    }

    pub fn with_interop_bridge_opt(mut self, interop_bridge: Option<Arc<DiscoveryBridge>>) -> Self {
        self.interop_bridge = interop_bridge;
        self
    }

    pub fn with_interop_metering(mut self, interop_metering: Arc<MeteringStore>) -> Self {
        self.interop_metering = Some(interop_metering);
        self
    }

    pub fn with_interop_metering_opt(
        mut self,
        interop_metering: Option<Arc<MeteringStore>>,
    ) -> Self {
        self.interop_metering = interop_metering;
        self
    }

    pub fn with_metrics(mut self, metrics: Arc<MetricsCollector>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn with_metrics_opt(mut self, metrics: Option<Arc<MetricsCollector>>) -> Self {
        self.metrics = metrics;
        self
    }

    pub fn with_session_cost_usd(mut self, cost: Arc<std::sync::atomic::AtomicU64>) -> Self {
        self.session_cost_usd = Some(cost);
        self
    }

    pub fn with_health_tracker_opt(
        mut self,
        health_tracker: Option<Arc<ProviderHealthTracker>>,
    ) -> Self {
        self.health_tracker = health_tracker;
        self
    }

    pub fn with_audit_service_opt(mut self, audit_service: Option<Arc<AuditService>>) -> Self {
        self.audit_service = audit_service;
        self
    }

    pub fn with_alias_cache_opt(mut self, alias_cache: Option<Arc<AliasCache>>) -> Self {
        self.alias_cache = alias_cache;
        self
    }

    pub fn with_alias_repo_opt(mut self, alias_repo: Option<Arc<AliasRepository>>) -> Self {
        self.alias_repo = alias_repo;
        self
    }

    pub fn with_uh_store(
        mut self,
        store: Option<Arc<uglyhat::store::sqlite::SqliteStore>>,
        workspace_id: Option<uuid::Uuid>,
    ) -> Self {
        self.uh_store = store;
        self.uh_workspace_id = workspace_id;
        self
    }

    pub fn with_pg_pool(mut self, pool: sqlx::PgPool) -> Self {
        self.pg_pool = Some(pool);
        self
    }

    pub fn with_pg_pool_opt(mut self, pool: Option<sqlx::PgPool>) -> Self {
        self.pg_pool = pool;
        self
    }

    // --- build() ---

    pub fn build(self) -> Result<AppState, AppStateBuildError> {
        let providers = self.providers.ok_or(AppStateBuildError::MissingProviders)?;
        let event_tx = self.event_tx.ok_or(AppStateBuildError::MissingEventTx)?;

        let fitness_ttl = self.config.routing.fitness.cache_ttl_secs;
        let routing_rules = self.config.routing.rules.clone();

        let fitness_cache = self
            .fitness_cache
            .unwrap_or_else(|| FitnessCache::new(fitness_ttl));
        let routing_policy = self
            .routing_policy
            .unwrap_or_else(|| crate::routing::policy::load_policy(routing_rules));
        let rate_limiter = self
            .rate_limiter
            .unwrap_or_else(|| Arc::new(RateLimiter::new()));
        let budget_tracker = self
            .budget_tracker
            .unwrap_or_else(|| Arc::new(BudgetTracker::new()));
        let session_tracker = self
            .session_tracker
            .unwrap_or_else(|| Arc::new(Mutex::new(SessionTracker::new())));

        Ok(AppState {
            config: self.config,
            providers,
            event_tx,
            http_client: reqwest::Client::new(),
            fitness_cache,
            routing_policy,
            rate_limiter,
            budget_tracker,
            session_tracker,
            key_service: self.key_service,
            experiment_engine: self.experiment_engine,
            response_cache: self.response_cache,
            feedback_tx: self.feedback_tx,
            benchmark_tx: self.benchmark_tx,
            mcp_tx: self.mcp_tx,
            hot_config: self.hot_config,
            hot_routing_policy: self.hot_routing_policy,
            prompt_store: self.prompt_store,
            callback_registry: self.callback_registry,
            interop_bridge: self.interop_bridge,
            interop_metering: self.interop_metering,
            metrics: self.metrics,
            session_cost_usd: self
                .session_cost_usd
                .unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicU64::new(0))),
            health_tracker: self.health_tracker,
            audit_service: self.audit_service,
            alias_cache: self.alias_cache,
            alias_repo: self.alias_repo,
            circuit_breakers: self
                .circuit_breakers
                .unwrap_or_else(new_circuit_breaker_map),
            session_spend: self
                .session_spend
                .unwrap_or_else(|| Arc::new(dashmap::DashMap::new())),
            uh_store: self.uh_store,
            uh_workspace_id: self.uh_workspace_id,
            pg_pool: self.pg_pool,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_config() -> Config {
        figment::Figment::new()
            .extract()
            .expect("default Config from empty figment")
    }

    fn test_providers() -> Arc<ProviderRegistry> {
        Arc::new(ProviderRegistry::from_config(
            &HashMap::new(),
            reqwest::Client::new(),
        ))
    }

    fn test_event_tx() -> tokio::sync::mpsc::Sender<InferenceEvent> {
        tokio::sync::mpsc::channel::<InferenceEvent>(1).0
    }

    #[test]
    fn build_fails_without_providers() {
        let result = AppStateBuilder::new(test_config())
            .with_event_tx(test_event_tx())
            .build();
        assert!(matches!(result, Err(AppStateBuildError::MissingProviders)));
    }

    #[test]
    fn build_fails_without_event_tx() {
        let result = AppStateBuilder::new(test_config())
            .with_providers(test_providers())
            .build();
        assert!(matches!(result, Err(AppStateBuildError::MissingEventTx)));
    }

    #[test]
    fn build_succeeds_with_only_required_fields() {
        let state = AppStateBuilder::new(test_config())
            .with_providers(test_providers())
            .with_event_tx(test_event_tx())
            .build()
            .expect("build with only required fields");

        assert!(state.key_service.is_none());
        assert!(state.experiment_engine.is_none());
        assert!(state.response_cache.is_none());
        assert!(state.feedback_tx.is_none());
        assert!(state.benchmark_tx.is_none());
        assert!(state.mcp_tx.is_none());
        assert!(state.hot_config.is_none());
        assert!(state.hot_routing_policy.is_none());
        assert!(state.prompt_store.is_none());
        assert!(state.callback_registry.is_none());
        assert!(state.interop_bridge.is_none());
        assert!(state.interop_metering.is_none());
        assert!(state.metrics.is_none());
        assert!(state.health_tracker.is_none());
        assert!(state.audit_service.is_none());
        assert!(state.alias_cache.is_none());
        assert!(state.alias_repo.is_none());
    }

    #[test]
    fn routing_policy_default_uses_config_rules() {
        let state = AppStateBuilder::new(test_config())
            .with_providers(test_providers())
            .with_event_tx(test_event_tx())
            .build()
            .unwrap();
        // Empty rules → build_default_policy() → at least one rule
        assert!(!state.routing_policy.rules.is_empty());
    }

    #[test]
    fn caller_overrides_take_precedence() {
        let custom_limiter = Arc::new(RateLimiter::new());
        let ptr = Arc::as_ptr(&custom_limiter);

        let state = AppStateBuilder::new(test_config())
            .with_providers(test_providers())
            .with_event_tx(test_event_tx())
            .with_rate_limiter(custom_limiter)
            .build()
            .unwrap();

        assert_eq!(Arc::as_ptr(&state.rate_limiter), ptr);
    }

    #[test]
    fn optional_field_with_value() {
        let m = Arc::new(MetricsCollector::new());
        let ptr = Arc::as_ptr(&m);

        let state = AppStateBuilder::new(test_config())
            .with_providers(test_providers())
            .with_event_tx(test_event_tx())
            .with_metrics(m)
            .build()
            .unwrap();

        assert!(state.metrics.is_some());
        assert_eq!(Arc::as_ptr(state.metrics.as_ref().unwrap()), ptr);
    }

    #[test]
    fn with_key_service_opt_none_stays_none() {
        let state = AppStateBuilder::new(test_config())
            .with_providers(test_providers())
            .with_event_tx(test_event_tx())
            .with_key_service_opt(None)
            .build()
            .unwrap();

        assert!(state.key_service.is_none());
    }
}
