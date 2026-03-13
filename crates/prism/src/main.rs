mod alerts;
mod api;
mod benchmark;
mod billing;
mod cache;
mod classifier;
mod compliance;
mod config;
mod enterprise;
mod error;
mod experiment;
mod finetuning;
mod guardrails;
mod interop;
mod keys;
mod mcp;
mod models;
mod observability;
mod optimization;
mod prompts;
mod providers;
mod proxy;
pub mod routing;
mod server;
mod types;
mod waste;
mod workflows;

use std::sync::Arc;
use std::time::Duration;

use tokio::signal;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::benchmark::judge::Judge;
use crate::benchmark::sampler::BenchmarkSampler;
use crate::cache::ResponseCache;
use crate::config::Config;
use crate::experiment::engine::ExperimentEngine;
use crate::experiment::feedback::FeedbackEvent;
use crate::keys::KeyService;
use crate::keys::audit::AuditService;
use crate::keys::budget::BudgetTracker;
use crate::keys::rate_limit::RateLimiter;
use crate::keys::virtual_key::KeyRepository;
use crate::mcp::types::McpCall;
use crate::mcp::writer::McpWriter;
use crate::models::alias::{AliasCache, AliasRepository};
use crate::observability::writer::{
    BenchmarkWriter, CompletionSampleWriter, FeedbackWriter, InferenceWriter,
};
use crate::providers::ProviderRegistry;
use crate::types::CompletionSample;
use crate::types::InferenceEvent;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load config first so logging can be configured from it
    let config = Config::load(None).map_err(|e| anyhow::anyhow!("config error: {e}"))?;

    // Initialize tracing — JSON, Loki, or plain text based on config
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "prism=info,tower_http=info".into());

    #[cfg(feature = "tracing-loki")]
    {
        if let Some(ref loki_url) = config.logging.loki_url {
            let mut builder = tracing_loki::builder().label("service", "prism")?;
            for (k, v) in &config.logging.loki_labels {
                builder = builder.extra_field(k, v)?;
            }
            let (loki_layer, loki_task) = builder.build_url(loki_url.parse()?)?;
            tokio::spawn(loki_task);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer().json())
                .with(loki_layer)
                .init();
        } else if config.logging.format == "json" {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(env_filter)
                .init();
        } else {
            tracing_subscriber::fmt().with_env_filter(env_filter).init();
        }
    }

    #[cfg(not(feature = "tracing-loki"))]
    if config.logging.format == "json" {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }

    // Initialize start time for health endpoint
    crate::api::health::init_start_time();

    // Initialize OpenTelemetry tracer (if enabled)
    #[cfg(feature = "otel")]
    if config.otel.enabled {
        if let Err(e) = observability::otel::init_tracer(&config.otel) {
            tracing::warn!(error = %e, "OTEL tracer init failed — continuing without traces");
        }
    }
    tracing::info!(
        address = %config.gateway.address,
        providers = ?config.providers.keys().collect::<Vec<_>>(),
        models = ?config.models.keys().collect::<Vec<_>>(),
        keys_enabled = config.keys.enabled,
        "starting prism"
    );

    // Shared cancellation token for graceful shutdown
    let cancel = CancellationToken::new();

    // Event channel: proxy handlers → inference writer
    let (event_tx, event_rx) = mpsc::channel::<InferenceEvent>(config.pipeline.queue_size);

    // HTTP client shared across providers
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.streaming.request_timeout_secs))
        .connect_timeout(Duration::from_secs(10))
        .build()?;

    // Build provider registry (wrapped in Arc for sharing with benchmark sampler)
    let registry = Arc::new(ProviderRegistry::from_config(
        &config.providers,
        http_client,
    ));

    // ClickHouse writer
    let writer = InferenceWriter::new(
        event_rx,
        &config.clickhouse,
        config.pipeline.batch_size,
        config.pipeline.flush_interval_ms,
        cancel.clone(),
    );

    // Apply ClickHouse schema (best-effort — ClickHouse may not be running in dev)
    if let Err(e) = writer.migrate().await {
        tracing::warn!(error = %e, "clickhouse migration failed (will retry on first write)");
    }

    // Spawn inference writer
    let writer_handle = tokio::spawn(writer.run());

    // Build routing components
    let fitness_cache = routing::FitnessCache::new(config.routing.fitness.cache_ttl_secs);
    let routing_policy = routing::policy::load_policy(config.routing.rules.clone());

    if config.routing.enabled {
        tracing::info!(rules = routing_policy.rules.len(), "routing enabled");
    }

    // --- Postgres + Virtual Keys ---
    let key_service: Option<Arc<KeyService>> = if config.keys.enabled {
        let pg_config = config
            .postgres
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("keys.enabled requires [postgres] config"))?;

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(pg_config.max_connections)
            .acquire_timeout(Duration::from_secs(pg_config.connect_timeout_secs))
            .connect(&pg_config.url)
            .await?;

        tracing::info!("connected to postgres");

        // Run migrations
        let migration_sql = include_str!("../migrations/postgres/001_virtual_keys.sql");
        sqlx::raw_sql(migration_sql).execute(&pool).await?;
        tracing::info!("postgres migrations applied");

        let repo = KeyRepository::new(pool);
        let ks = Arc::new(KeyService::new(repo, config.keys.cache_capacity));
        Some(ks)
    } else {
        None
    };

    // --- Apply additional Postgres migrations (best-effort) ---
    if let Some(ref ks) = key_service {
        let pool = ks.repo().pool();
        let prompts_sql = include_str!("../migrations/postgres/002_prompts.sql");
        if let Err(e) = sqlx::raw_sql(prompts_sql).execute(pool).await {
            tracing::warn!(error = %e, "002_prompts migration failed (may already exist)");
        }
        let budget_sql = include_str!("../migrations/postgres/003_budget_hierarchy.sql");
        if let Err(e) = sqlx::raw_sql(budget_sql).execute(pool).await {
            tracing::warn!(error = %e, "003_budget_hierarchy migration failed (may already exist)");
        }
        let rotation_sql = include_str!("../migrations/postgres/004_key_rotation.sql");
        if let Err(e) = sqlx::raw_sql(rotation_sql).execute(pool).await {
            tracing::warn!(error = %e, "004_key_rotation migration failed (may already exist)");
        }
        let tenant_users_sql = include_str!("../migrations/postgres/005_tenant_users.sql");
        if let Err(e) = sqlx::raw_sql(tenant_users_sql).execute(pool).await {
            tracing::warn!(error = %e, "005_tenant_users migration failed (may already exist)");
        }
        let aliases_sql = include_str!("../migrations/postgres/006_model_aliases.sql");
        if let Err(e) = sqlx::raw_sql(aliases_sql).execute(pool).await {
            tracing::warn!(error = %e, "006_model_aliases migration failed (may already exist)");
        }
        let audit_sql = include_str!("../migrations/postgres/007_audit_events.sql");
        if let Err(e) = sqlx::raw_sql(audit_sql).execute(pool).await {
            tracing::warn!(error = %e, "007_audit_events migration failed (may already exist)");
        }
        let rotation_sched_sql = include_str!("../migrations/postgres/008_rotation_scheduler.sql");
        if let Err(e) = sqlx::raw_sql(rotation_sched_sql).execute(pool).await {
            tracing::warn!(error = %e, "008_rotation_scheduler migration failed (may already exist)");
        }
        let ip_cors_sql = include_str!("../migrations/postgres/009_key_ip_cors.sql");
        if let Err(e) = sqlx::raw_sql(ip_cors_sql).execute(pool).await {
            tracing::warn!(error = %e, "009_key_ip_cors migration failed (may already exist)");
        }
        let debug_sql = include_str!("../migrations/postgres/010_debug_sessions.sql");
        if let Err(e) = sqlx::raw_sql(debug_sql).execute(pool).await {
            tracing::warn!(error = %e, "010_debug_sessions migration failed (may already exist)");
        }
        tracing::info!("additional postgres migrations applied");
    }

    // --- Rate Limiter (memory or redis) + Budget Tracker ---
    let rate_limiter = Arc::new(match config.rate_limit.backend.as_str() {
        "redis" => {
            let url = config
                .rate_limit
                .redis_url
                .as_deref()
                .unwrap_or("redis://127.0.0.1:6379");
            tracing::info!(url = url, "using redis rate limiter");
            RateLimiter::new_redis(url).await
        }
        _ => RateLimiter::new(),
    });
    let budget_tracker = Arc::new(BudgetTracker::new());

    // Spawn rate limiter pruning task (every 60s)
    {
        let rl = rate_limiter.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {
                        rl.prune_expired();
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    // Spawn budget reconciliation loop (queries ClickHouse for daily/monthly spend per key_hash)
    {
        let bt = budget_tracker.clone();
        let cancel = cancel.clone();
        let interval_secs = config.keys.budget_reconcile_interval_secs;
        let ch_url = config.clickhouse.url.clone();
        let ch_db = config.clickhouse.database.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {
                        if let Err(e) = reconcile_budgets(&bt, &ch_url, &ch_db).await {
                            tracing::warn!(error = %e, "budget reconciliation failed");
                        }
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    // --- Experiment Engine ---
    let experiment_engine = if config.experiments.enabled {
        tracing::info!(
            experiments = config.experiments.experiments.len(),
            "experimentation enabled"
        );
        Some(Arc::new(ExperimentEngine::new()))
    } else {
        None
    };

    // Spawn episode assignment pruning task (every 300s)
    if let Some(ref engine) = experiment_engine {
        let engine = engine.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(300)) => {
                        engine.prune_episodes();
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    // --- Response Cache (memory, redis, or s3) ---
    let response_cache = if config.cache.enabled {
        let cache = match config.cache.backend.as_str() {
            "redis" => {
                let url = config
                    .cache
                    .redis_url
                    .as_deref()
                    .or(config.rate_limit.redis_url.as_deref())
                    .unwrap_or("redis://127.0.0.1:6379");
                tracing::info!(
                    url = url,
                    ttl_secs = config.cache.ttl_secs,
                    "redis cache enabled"
                );
                ResponseCache::new_redis(url, config.cache.ttl_secs).await
            }
            "s3" => {
                let bucket = config.cache.s3_bucket.as_deref().unwrap_or("prism-cache");
                let prefix = config.cache.s3_prefix.as_deref().unwrap_or("cache/");
                tracing::info!(bucket = bucket, prefix = prefix, "s3 cache enabled");
                ResponseCache::new_s3(bucket, prefix, config.cache.ttl_secs).await
            }
            _ => {
                tracing::info!(
                    max_size = config.cache.max_size,
                    ttl_secs = config.cache.ttl_secs,
                    "in-memory response cache enabled"
                );
                ResponseCache::new(config.cache.max_size, config.cache.ttl_secs)
            }
        };
        Some(Arc::new(cache))
    } else {
        None
    };

    // Spawn cache pruning task (every 60s)
    if let Some(ref cache) = response_cache {
        let cache = cache.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {
                        cache.prune_expired();
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    // --- Feedback channel + writer ---
    let (feedback_tx, feedback_rx) = mpsc::channel::<FeedbackEvent>(config.pipeline.queue_size);

    let feedback_writer = FeedbackWriter::new(
        feedback_rx,
        &config.clickhouse,
        config.pipeline.batch_size,
        config.pipeline.flush_interval_ms,
        cancel.clone(),
    );
    let feedback_writer_handle = tokio::spawn(feedback_writer.run());

    // --- Benchmark pipeline ---
    let mut benchmark_tx_option: Option<
        tokio::sync::mpsc::Sender<crate::benchmark::BenchmarkRequest>,
    > = None;
    let mut benchmark_writer_handle: Option<tokio::task::JoinHandle<()>> = None;

    if config.benchmark.enabled {
        tracing::info!(
            sample_rate = config.benchmark.sample_rate,
            judge_model = %config.benchmark.judge_model,
            "benchmarking enabled"
        );

        let (bench_req_tx, bench_req_rx) =
            mpsc::channel::<crate::benchmark::BenchmarkRequest>(config.pipeline.queue_size);
        let (bench_event_tx, bench_event_rx) =
            mpsc::channel::<crate::benchmark::BenchmarkEvent>(config.pipeline.queue_size);

        // Spawn BenchmarkWriter (events → ClickHouse)
        let bw = BenchmarkWriter::new(
            bench_event_rx,
            &config.clickhouse,
            config.pipeline.batch_size,
            config.pipeline.flush_interval_ms,
            cancel.clone(),
        );
        benchmark_writer_handle = Some(tokio::spawn(bw.run()));

        // Spawn BenchmarkSampler
        let judge = Judge::new(config.benchmark.judge_model.clone());
        let sampler = BenchmarkSampler::new(
            bench_req_rx,
            bench_event_tx,
            config.clone(),
            registry.clone(),
            judge,
            cancel.clone(),
            fitness_cache.clone(),
        );
        tokio::spawn(sampler.run());

        // Spawn fitness refresh loop
        {
            let fitness_cache = fitness_cache.clone();
            let cancel = cancel.clone();
            let interval_secs = config.benchmark.fitness_refresh_interval_secs;
            let min_sample_size = config.benchmark.min_sample_size;
            let ch_url = config.clickhouse.url.clone();
            let ch_db = config.clickhouse.database.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {
                            if let Err(e) = crate::benchmark::refresh::refresh_fitness_from_benchmarks(
                                &fitness_cache,
                                &ch_url,
                                &ch_db,
                                min_sample_size,
                            ).await {
                                tracing::warn!(error = %e, "fitness refresh from benchmarks failed");
                            }
                        }
                        _ = cancel.cancelled() => break,
                    }
                }
            });
        }

        benchmark_tx_option = Some(bench_req_tx);
    }

    // --- Traffic-based fitness refresh (cost + latency from live inference_events) ---
    if !config.clickhouse.url.is_empty() {
        let fc = fitness_cache.clone();
        let ch_url = config.clickhouse.url.clone();
        let ch_db = config.clickhouse.database.clone();
        let cancel = cancel.clone();
        let interval_secs = config.benchmark.traffic_fitness_refresh_interval_secs;
        let min_samples = config.benchmark.traffic_fitness_min_samples;
        let lookback_days = config.benchmark.traffic_fitness_lookback_days;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {
                        if let Err(e) = routing::traffic::refresh_fitness_from_traffic(
                            &fc, &ch_url, &ch_db, min_samples, lookback_days,
                        ).await {
                            tracing::warn!(error = %e, "traffic fitness refresh failed");
                        }
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    // --- Live judge completion sampling pipeline ---
    let mut completion_sample_tx_option: Option<tokio::sync::mpsc::Sender<CompletionSample>> = None;
    let mut completion_sample_writer_handle: Option<tokio::task::JoinHandle<()>> = None;

    if config.benchmark.live_judge_enabled && !config.clickhouse.url.is_empty() {
        tracing::info!(
            interval_secs = config.benchmark.live_judge_interval_secs,
            max_calls_per_minute = config.benchmark.live_judge_max_calls_per_minute,
            sample_rate = config.benchmark.live_judge_sample_rate,
            judge_model = %config.benchmark.judge_model,
            "live judge feedback loop enabled"
        );

        let (sample_tx, sample_rx) = mpsc::channel::<CompletionSample>(config.pipeline.queue_size);

        let csw = CompletionSampleWriter::new(
            sample_rx,
            &config.clickhouse,
            config.pipeline.batch_size,
            config.pipeline.flush_interval_ms,
            cancel.clone(),
        );
        completion_sample_writer_handle = Some(tokio::spawn(csw.run()));
        completion_sample_tx_option = Some(sample_tx);

        let live_judge = routing::live_judge::LiveJudgeTask::new(
            registry.clone(),
            config.clone(),
            fitness_cache.clone(),
            cancel.clone(),
            config.benchmark.live_judge_interval_secs,
            config.benchmark.live_judge_max_calls_per_minute,
            config.benchmark.live_judge_lookback_secs,
            config.clickhouse.url.clone(),
            config.clickhouse.database.clone(),
        );
        tokio::spawn(live_judge.run());
    }

    // --- Feedback Adjuster ---
    if config.feedback_adjuster.enabled {
        tracing::info!(
            interval_secs = config.feedback_adjuster.interval_secs,
            alpha = config.feedback_adjuster.alpha,
            "feedback adjuster enabled"
        );

        let fitness_cache = fitness_cache.clone();
        let cancel = cancel.clone();
        let interval_secs = config.feedback_adjuster.interval_secs;
        let alpha = config.feedback_adjuster.alpha;
        let min_samples = config.feedback_adjuster.min_samples;
        let max_adjustment = config.feedback_adjuster.max_adjustment;
        let ch_url = config.clickhouse.url.clone();
        let ch_db = config.clickhouse.database.clone();

        tokio::spawn(async move {
            use crate::experiment::adjuster;
            let mut state = adjuster::FeedbackAdjusterState::default();

            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {
                        match adjuster::query_feedback_deltas(&ch_url, &ch_db, min_samples).await {
                            Ok(rows) if !rows.is_empty() => {
                                let adjustments = adjuster::compute_adjustments(
                                    &rows, &mut state, alpha, min_samples, max_adjustment,
                                );
                                adjuster::apply_adjustments(&fitness_cache, &adjustments).await;
                                tracing::debug!(
                                    adjustments = adjustments.len(),
                                    "applied feedback adjustments"
                                );
                            }
                            Ok(_) => {} // no feedback data
                            Err(e) => {
                                tracing::warn!(error = %e, "feedback adjuster query failed");
                            }
                        }
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    // --- MCP Writer ---
    let (mcp_tx, mcp_rx) = mpsc::channel::<McpCall>(config.pipeline.queue_size);

    let mcp_writer = McpWriter::new(
        mcp_rx,
        &config.clickhouse,
        config.pipeline.batch_size,
        config.pipeline.flush_interval_ms,
        cancel.clone(),
    );
    let mcp_writer_handle = tokio::spawn(mcp_writer.run());

    // --- Alert Checker ---
    if config.alerts.enabled && !config.alerts.rules.is_empty() {
        let rules: Vec<crate::alerts::types::AlertRule> = config
            .alerts
            .rules
            .iter()
            .filter_map(|r| {
                let rule_type = match r.rule_type.as_str() {
                    "spend_threshold" => crate::alerts::types::RuleType::SpendThreshold,
                    "anomaly_zscore" => crate::alerts::types::RuleType::AnomalyZscore,
                    "error_rate" => crate::alerts::types::RuleType::ErrorRate,
                    "latency_p95" => crate::alerts::types::RuleType::LatencyP95,
                    other => {
                        tracing::warn!(rule_type = other, "unknown alert rule type, skipping");
                        return None;
                    }
                };
                let channel = match r.channel.as_str() {
                    "webhook" => crate::alerts::types::AlertChannel::Webhook,
                    "slack" => crate::alerts::types::AlertChannel::Slack,
                    "email" => crate::alerts::types::AlertChannel::Email,
                    _ => crate::alerts::types::AlertChannel::Log,
                };
                Some(crate::alerts::types::AlertRule {
                    id: r.id,
                    rule_type,
                    threshold: r.threshold,
                    channel,
                    webhook_url: r.webhook_url.clone(),
                    slack_webhook_url: r.slack_webhook_url.clone(),
                    email_to: r.email_to.clone(),
                    enabled: r.enabled,
                })
            })
            .collect();

        if !rules.is_empty() {
            tracing::info!(rules = rules.len(), "alert checker enabled");

            let checker = crate::alerts::checker::AlertChecker::new(
                rules,
                config.clickhouse.url.clone(),
                config.clickhouse.database.clone(),
                config.alerts.check_interval_secs,
                config.alerts.cooldown_secs,
                cancel.clone(),
                config.smtp.clone(),
            );
            tokio::spawn(checker.run());
        }
    }

    // --- Budget Watcher ---
    #[cfg(feature = "postgres")]
    if config.keys.budget_alerts.enabled {
        let budget_repo = key_service.as_deref().map(|ks| ks.repo().clone());
        if let Some(repo) = budget_repo {
            tracing::info!(
                warn_pct = config.keys.budget_alerts.warn_threshold_pct,
                interval_secs = config.keys.budget_alerts.check_interval_secs,
                "budget alerting enabled"
            );
            let watcher = crate::alerts::budget_watcher::BudgetWatcher::new(
                config.keys.budget_alerts.clone(),
                budget_tracker.clone(),
                repo,
                config.smtp.clone(),
            );
            let cancel = cancel.clone();
            tokio::spawn(watcher.run(cancel));
        }
    }

    // --- Session Tracker ---
    let session_tracker = Arc::new(tokio::sync::Mutex::new(
        crate::routing::session::SessionTracker::new(),
    ));

    // Spawn session pruning task (every 300s)
    {
        let tracker = session_tracker.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(300)) => {
                        tracker.lock().await.prune();
                    }
                    _ = cancel.cancelled() => break,
                }
            }
        });
    }

    // --- Prompt Store (in-memory or Postgres) ---
    let prompt_store = if let Some(ref ks) = key_service {
        let pool = ks.repo().pool().clone();
        tracing::info!("using postgres prompt store");
        Arc::new(crate::prompts::store::PromptStore::new_postgres(pool))
    } else {
        Arc::new(crate::prompts::store::PromptStore::new())
    };

    // --- Observability Callbacks ---
    let callback_registry = {
        let mut registry = crate::observability::callbacks::CallbackRegistry::new();
        if let Some(ref lf) = config.observability_callbacks.langfuse {
            registry.register(Box::new(
                crate::observability::callbacks::langfuse::LangfuseCallback::new(
                    lf.api_url.clone(),
                    lf.public_key.clone(),
                    lf.secret_key.clone(),
                ),
            ));
        }
        if let Some(ref hc) = config.observability_callbacks.helicone {
            registry.register(Box::new(
                crate::observability::callbacks::helicone::HeliconeCallback::new(
                    hc.api_key.clone(),
                    hc.api_url.clone(),
                ),
            ));
        }
        if let Some(ref dd) = config.observability_callbacks.datadog {
            registry.register(Box::new(
                crate::observability::callbacks::datadog::DatadogCallback::new(
                    dd.api_key.clone(),
                    dd.site.clone(),
                ),
            ));
        }
        if !registry.is_empty() {
            tracing::info!("observability callbacks registered");
        }
        Some(Arc::new(registry))
    };

    // --- Interop Components ---
    let (interop_bridge, interop_metering) = if config.interop.enabled {
        tracing::info!("cross-platform interop enabled");
        (
            Some(Arc::new(crate::interop::bridge::DiscoveryBridge::new())),
            Some(Arc::new(crate::interop::metering::MeteringStore::new())),
        )
    } else {
        (None, None)
    };

    // --- Metrics Collector ---
    let metrics_collector = Arc::new(crate::observability::metrics::MetricsCollector::new());
    tracing::info!("prometheus metrics enabled at /metrics");

    // --- Phase 4: AuditService, AliasRepository/Cache, ProviderHealthTracker ---
    #[cfg(feature = "postgres")]
    let (audit_service, alias_repo, alias_cache) = if let Some(ref ks) = key_service {
        let pool = ks.repo().pool().clone();

        let audit_svc = Arc::new(AuditService::new(pool.clone()));
        let alias_repository = Arc::new(AliasRepository::new(pool.clone()));
        let alias_cache_instance = AliasCache::new();

        // Pre-load all DB aliases into cache
        match alias_repository.load_all_pairs().await {
            Ok(pairs) => {
                alias_cache_instance.load_all(pairs).await;
                tracing::info!("model alias cache loaded");
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to load model aliases into cache");
            }
        }

        (
            Some(audit_svc),
            Some(alias_repository),
            Some(alias_cache_instance),
        )
    } else {
        (None, None, None)
    };

    #[cfg(not(feature = "postgres"))]
    let (audit_service, alias_repo, alias_cache): (
        Option<Arc<AuditService>>,
        Option<Arc<AliasRepository>>,
        Option<Arc<AliasCache>>,
    ) = (None, None, None);

    // Provider health tracker
    let health_tracker = Arc::new(crate::providers::health::ProviderHealthTracker::new(3));
    {
        let tracker = health_tracker.clone();
        let reg = registry.clone();
        let hc = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        let cancel = cancel.clone();
        tokio::spawn(crate::providers::health::spawn_health_checker(
            tracker, reg, hc, 30, cancel,
        ));
    }

    // Rotation scheduler — runs every hour, rotates due keys, logs audit events
    // Clone what we need before key_service is moved into AppState.
    #[cfg(feature = "postgres")]
    {
        let rotation_repo: Option<KeyRepository> =
            key_service.as_deref().map(|ks| ks.repo().clone());
        let rotation_audit = audit_service.clone();
        if let (Some(repo), Some(audit)) = (rotation_repo, rotation_audit) {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(3600)) => {
                            rotate_due_keys(&repo, &audit).await;
                        }
                        _ = cancel.cancelled() => break,
                    }
                }
            });
        }
    }

    // Open context SQLite store for agent inbox/handoff APIs (best-effort).
    // Discovers .prism/context.json by walking up from the current working directory.
    let (context_store_arc, context_workspace_id) = {
        use prism_context::config as uh_config;
        use prism_context::store::sqlite::SqliteStore;

        let result: (Option<Arc<SqliteStore>>, Option<uuid::Uuid>) = async {
            let cwd = std::env::current_dir().ok()?;
            let cfg_path = uh_config::find_config(&cwd)?;
            let cfg = uh_config::load_config(&cfg_path).ok()?;
            let ws_id = cfg.workspace_id.parse::<uuid::Uuid>().ok()?;
            let db_path = uh_config::resolve_db_path(&cfg_path, &cfg);
            match SqliteStore::open(&db_path.to_string_lossy()).await {
                Ok(store) => {
                    tracing::info!(path = %db_path.display(), "context store opened");
                    Some((Arc::new(store), ws_id))
                }
                Err(e) => {
                    tracing::debug!(error = %e, "context store open failed");
                    None
                }
            }
        }
        .await
        .map(|(s, w)| (Some(s), Some(w)))
        .unwrap_or((None, None));
        result
    };

    // Expose the pool directly on AppState so features like debug sessions don't have
    // to route through key_service, which isn't present in all deployments.
    let pg_pool = key_service.as_ref().map(|ks| ks.repo().pool().clone());

    // Build app state
    let state = Arc::new(
        crate::proxy::AppStateBuilder::new(config.clone())
            .with_providers(registry)
            .with_event_tx(event_tx)
            .with_fitness_cache(fitness_cache)
            .with_routing_policy(routing_policy.clone())
            .with_rate_limiter(rate_limiter)
            .with_budget_tracker(budget_tracker)
            .with_session_tracker(session_tracker)
            .with_key_service_opt(key_service)
            .with_experiment_engine_opt(experiment_engine)
            .with_response_cache_opt(response_cache)
            .with_feedback_tx(feedback_tx)
            .with_benchmark_tx_opt(benchmark_tx_option)
            .with_completion_sample_tx_opt(completion_sample_tx_option)
            .with_mcp_tx(mcp_tx)
            .with_prompt_store(prompt_store)
            .with_hot_config(Arc::new(arc_swap::ArcSwap::from_pointee(config.clone())))
            .with_hot_routing_policy(Arc::new(arc_swap::ArcSwap::from_pointee(
                routing_policy.clone(),
            )))
            .with_callback_registry_opt(callback_registry)
            .with_interop_bridge_opt(interop_bridge)
            .with_interop_metering_opt(interop_metering)
            .with_metrics(metrics_collector)
            .with_health_tracker_opt(Some(health_tracker))
            .with_audit_service_opt(audit_service)
            .with_alias_cache_opt(alias_cache)
            .with_alias_repo_opt(alias_repo)
            .with_context_store(context_store_arc, context_workspace_id)
            .with_pg_pool_opt(pg_pool)
            .build()
            .expect("AppState construction is infallible after main.rs init"),
    );

    // Build router
    let app = server::router::build(state);

    // Bind and serve
    let listener = tokio::net::TcpListener::bind(&config.gateway.address).await?;
    tracing::info!(address = %config.gateway.address, "prism listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(cancel.clone()))
        .await?;

    // Shutdown: cancel all workers, wait for drain
    cancel.cancel();
    tracing::info!("shutting down workers...");

    // Wait for writers to flush remaining events (with timeout)
    tokio::select! {
        _ = async {
            let _ = writer_handle.await;
            let _ = feedback_writer_handle.await;
            let _ = mcp_writer_handle.await;
            if let Some(handle) = benchmark_writer_handle {
                let _ = handle.await;
            }
            if let Some(handle) = completion_sample_writer_handle {
                let _ = handle.await;
            }
        } => {}
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            tracing::warn!("writers did not shut down in time");
        }
    }

    // Shutdown OpenTelemetry tracer provider
    #[cfg(feature = "otel")]
    observability::otel::shutdown();

    tracing::info!("prism stopped");
    Ok(())
}

/// Reconcile in-memory budget state with ClickHouse daily/monthly totals.
async fn reconcile_budgets(
    tracker: &BudgetTracker,
    ch_url: &str,
    ch_db: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    // Query daily spend per key from ClickHouse
    let query = format!(
        "SELECT virtual_key_hash, sum(estimated_cost_usd) as total \
         FROM {db}.inference_events \
         WHERE toDate(timestamp) = today() AND virtual_key_hash != '' \
         GROUP BY virtual_key_hash \
         FORMAT JSONEachRow",
        db = ch_db
    );

    let daily_resp = client.post(ch_url).body(query).send().await?.text().await?;

    // Query monthly spend per key
    let query = format!(
        "SELECT virtual_key_hash, sum(estimated_cost_usd) as total \
         FROM {db}.inference_events \
         WHERE toStartOfMonth(timestamp) = toStartOfMonth(now()) AND virtual_key_hash != '' \
         GROUP BY virtual_key_hash \
         FORMAT JSONEachRow",
        db = ch_db
    );

    let monthly_resp = client.post(ch_url).body(query).send().await?.text().await?;

    // Parse and reconcile
    let mut daily_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for line in daily_resp.lines() {
        if let Ok(row) = serde_json::from_str::<serde_json::Value>(line)
            && let (Some(key), Some(total)) = (
                row.get("virtual_key_hash").and_then(|v| v.as_str()),
                row.get("total").and_then(|v| v.as_f64()),
            )
        {
            daily_map.insert(key.to_string(), total);
        }
    }

    let mut monthly_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for line in monthly_resp.lines() {
        if let Ok(row) = serde_json::from_str::<serde_json::Value>(line)
            && let (Some(key), Some(total)) = (
                row.get("virtual_key_hash").and_then(|v| v.as_str()),
                row.get("total").and_then(|v| v.as_f64()),
            )
        {
            monthly_map.insert(key.to_string(), total);
        }
    }

    // Collect all keys
    let all_keys: std::collections::HashSet<&String> =
        daily_map.keys().chain(monthly_map.keys()).collect();

    for key_hash in all_keys {
        let daily = daily_map.get(key_hash).copied().unwrap_or(0.0);
        let monthly = monthly_map.get(key_hash).copied().unwrap_or(0.0);
        tracker.reconcile(key_hash, daily, monthly);
    }

    tracing::debug!(keys = daily_map.len(), "budget reconciliation complete");

    Ok(())
}

/// Rotate all virtual keys whose rotation interval has elapsed.
#[cfg(feature = "postgres")]
async fn rotate_due_keys(repo: &KeyRepository, audit: &AuditService) {
    match repo.find_keys_due_for_rotation().await {
        Ok(due_keys) => {
            for vk in due_keys {
                let key_id = vk.id;
                let new_plaintext = crate::keys::generate_key();
                let new_hash = crate::keys::hash_key(&new_plaintext);
                let new_prefix = new_plaintext[..10].to_string();
                match repo.rotate_key(key_id, &new_hash, &new_prefix, 24).await {
                    Ok(Some(new_key)) => {
                        tracing::info!(
                            key_id = %key_id,
                            new_key_id = %new_key.id,
                            "rotation scheduler: rotated key"
                        );
                        audit.log(
                            crate::keys::audit::AuditEventType::KeyRotated,
                            Some(key_id),
                            None,
                            Some("rotation_scheduler".to_string()),
                            serde_json::json!({
                                "old_key_id": key_id,
                                "new_key_id": new_key.id,
                                "new_key_prefix": new_prefix,
                            }),
                            None,
                        );
                    }
                    Ok(None) => {
                        tracing::warn!(key_id = %key_id, "rotation scheduler: key not found");
                    }
                    Err(e) => {
                        tracing::warn!(
                            key_id = %key_id,
                            error = %e,
                            "rotation scheduler: rotate_key failed"
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "rotation scheduler: find_keys_due_for_rotation failed");
        }
    }
}

async fn shutdown_signal(cancel: CancellationToken) {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to listen for ctrl+c");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to listen for SIGTERM")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
        _ = cancel.cancelled() => {},
    }

    tracing::info!("shutdown signal received");
}
