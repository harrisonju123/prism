use figment::{
    Figment,
    providers::{Env, Format, Toml},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::routing::types::RoutingRule;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_gateway")]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub clickhouse: ClickHouseConfig,
    #[serde(default)]
    pub postgres: Option<PostgresConfig>,
    #[serde(default)]
    pub keys: KeysConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,
    #[serde(default)]
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub experiments: ExperimentationConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub benchmark: BenchmarkConfig,
    #[serde(default)]
    pub waste: WasteConfig,
    #[serde(default)]
    pub feedback_adjuster: FeedbackAdjusterConfig,
    #[serde(default)]
    pub alerts: AlertConfig,
    #[serde(default)]
    pub smtp: Option<SmtpConfig>,
    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub jwt: JwtConfig,
    #[serde(default)]
    pub otel: OtelConfig,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
    #[serde(default)]
    pub billing: BillingConfig,
    #[serde(default)]
    pub interop: InteropConfig,
    #[serde(default)]
    pub observability_callbacks: ObservabilityCallbacksConfig,
    #[serde(default)]
    pub budget_hierarchy: BudgetHierarchyConfig,
    #[serde(default)]
    pub dashboard: DashboardConfig,
    #[serde(default)]
    pub cors: CorsConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    #[serde(default = "default_address")]
    pub address: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClickHouseConfig {
    #[serde(default = "default_clickhouse_url")]
    pub url: String,
    #[serde(default = "default_clickhouse_db")]
    pub database: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub provider_type: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelConfig {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub tier: Option<u8>,
    #[serde(default)]
    pub fallback: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub fallback_providers: Vec<FallbackProvider>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FallbackProvider {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PipelineConfig {
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_flush_interval_ms")]
    pub flush_interval_ms: u64,
    #[serde(default = "default_queue_size")]
    pub queue_size: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RoutingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_classifier_confidence_threshold")]
    pub classifier_confidence_threshold: f64,
    #[serde(default = "default_tier1_confidence_threshold")]
    pub tier1_confidence_threshold: f64,
    #[serde(default)]
    pub fitness: FitnessConfig,
    #[serde(default)]
    pub rules: Vec<RoutingRule>,
    #[serde(default)]
    pub llm_classifier: LlmClassifierConfig,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            classifier_confidence_threshold: default_classifier_confidence_threshold(),
            tier1_confidence_threshold: default_tier1_confidence_threshold(),
            fitness: FitnessConfig::default(),
            rules: Vec::new(),
            llm_classifier: LlmClassifierConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LlmClassifierConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_llm_classifier_model")]
    pub model: String,
    #[serde(default = "default_llm_classifier_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for LlmClassifierConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: default_llm_classifier_model(),
            timeout_ms: default_llm_classifier_timeout_ms(),
        }
    }
}

fn default_llm_classifier_model() -> String {
    "groq/gemma2-9b-it".to_string()
}

fn default_llm_classifier_timeout_ms() -> u64 {
    2000
}

#[derive(Debug, Clone, Deserialize)]
pub struct FitnessConfig {
    #[serde(default = "default_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
}

impl Default for FitnessConfig {
    fn default() -> Self {
        Self {
            cache_ttl_secs: default_cache_ttl_secs(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostgresConfig {
    pub url: String,
    #[serde(default = "default_pg_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_pg_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
}

fn default_pg_max_connections() -> u32 {
    10
}

fn default_pg_connect_timeout_secs() -> u64 {
    5
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeysConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub master_key: Option<String>,
    #[serde(default = "default_cache_capacity")]
    pub cache_capacity: usize,
    #[serde(default = "default_budget_reconcile_interval_secs")]
    pub budget_reconcile_interval_secs: u64,
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            master_key: None,
            cache_capacity: default_cache_capacity(),
            budget_reconcile_interval_secs: default_budget_reconcile_interval_secs(),
        }
    }
}

fn default_cache_capacity() -> usize {
    10_000
}

fn default_budget_reconcile_interval_secs() -> u64 {
    300
}

fn default_classifier_confidence_threshold() -> f64 {
    0.4
}

fn default_tier1_confidence_threshold() -> f64 {
    0.7
}

fn default_cache_ttl_secs() -> u64 {
    300
}

// --- Experimentation ---

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExperimentationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub experiments: HashMap<String, Experiment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Experiment {
    pub function_name: String,
    #[serde(default = "default_experiment_mode")]
    pub mode: ExperimentMode,
    #[serde(default)]
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentMode {
    Static,
    Bandit,
}

fn default_experiment_mode() -> ExperimentMode {
    ExperimentMode::Static
}

#[derive(Debug, Clone, Deserialize)]
pub struct Variant {
    pub name: String,
    pub model: String,
    #[serde(default = "default_variant_weight")]
    pub weight: f64,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system_prompt_prefix: Option<String>,
}

fn default_variant_weight() -> f64 {
    1.0
}

// --- Benchmark ---

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: f64,
    #[serde(default = "default_judge_model")]
    pub judge_model: String,
    #[serde(default = "default_max_benchmark_models")]
    pub max_benchmark_models: usize,
    #[serde(default = "default_max_concurrent_benchmarks")]
    pub max_concurrent_benchmarks: usize,
    #[serde(default = "default_fitness_refresh_interval_secs")]
    pub fitness_refresh_interval_secs: u64,
    #[serde(default = "default_min_sample_size")]
    pub min_sample_size: u32,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sample_rate: default_sample_rate(),
            judge_model: default_judge_model(),
            max_benchmark_models: default_max_benchmark_models(),
            max_concurrent_benchmarks: default_max_concurrent_benchmarks(),
            fitness_refresh_interval_secs: default_fitness_refresh_interval_secs(),
            min_sample_size: default_min_sample_size(),
        }
    }
}

fn default_sample_rate() -> f64 {
    0.05
}
fn default_judge_model() -> String {
    "gpt-4o-mini".to_string()
}
fn default_max_benchmark_models() -> usize {
    3
}
fn default_max_concurrent_benchmarks() -> usize {
    5
}
fn default_fitness_refresh_interval_secs() -> u64 {
    300
}
fn default_min_sample_size() -> u32 {
    10
}

// --- Alerts ---

#[derive(Debug, Clone, Deserialize)]
pub struct AlertConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_alert_check_interval_secs")]
    pub check_interval_secs: u64,
    #[serde(default = "default_alert_cooldown_secs")]
    pub cooldown_secs: u64,
    #[serde(default)]
    pub rules: Vec<AlertRuleConfig>,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            check_interval_secs: default_alert_check_interval_secs(),
            cooldown_secs: default_alert_cooldown_secs(),
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlertRuleConfig {
    #[serde(default = "uuid::Uuid::new_v4")]
    pub id: uuid::Uuid,
    pub rule_type: String,
    pub threshold: f64,
    #[serde(default = "default_alert_channel")]
    pub channel: String,
    pub webhook_url: Option<String>,
    pub slack_webhook_url: Option<String>,
    pub email_to: Option<String>,
    #[serde(default = "default_alert_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    #[serde(default = "default_smtp_port")]
    pub port: u16,
    #[serde(default = "default_smtp_from")]
    pub from_address: String,
}

fn default_smtp_port() -> u16 {
    587
}

fn default_smtp_from() -> String {
    "alerts@prism.gateway".into()
}

fn default_alert_check_interval_secs() -> u64 {
    60
}
fn default_alert_cooldown_secs() -> u64 {
    3600
}
fn default_alert_channel() -> String {
    "log".into()
}
fn default_alert_enabled() -> bool {
    true
}

// --- Feedback Adjuster ---

#[derive(Debug, Clone, Deserialize)]
pub struct FeedbackAdjusterConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_feedback_interval_secs")]
    pub interval_secs: u64,
    #[serde(default = "default_feedback_alpha")]
    pub alpha: f64,
    #[serde(default = "default_feedback_min_samples")]
    pub min_samples: u32,
    #[serde(default = "default_feedback_max_adjustment")]
    pub max_adjustment: f64,
}

impl Default for FeedbackAdjusterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_feedback_interval_secs(),
            alpha: default_feedback_alpha(),
            min_samples: default_feedback_min_samples(),
            max_adjustment: default_feedback_max_adjustment(),
        }
    }
}

fn default_feedback_interval_secs() -> u64 {
    600
}
fn default_feedback_alpha() -> f64 {
    0.05
}
fn default_feedback_min_samples() -> u32 {
    20
}
fn default_feedback_max_adjustment() -> f64 {
    0.15
}

// --- JWT ---

#[derive(Debug, Clone, Deserialize)]
pub struct JwtConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_jwt_algorithm")]
    pub algorithm: String,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub public_key_pem: Option<String>,
    #[serde(default)]
    pub issuer: Option<String>,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            algorithm: default_jwt_algorithm(),
            secret: None,
            public_key_pem: None,
            issuer: None,
        }
    }
}

fn default_jwt_algorithm() -> String {
    "HS256".into()
}

// --- OpenTelemetry ---

#[derive(Debug, Clone, Deserialize)]
pub struct OtelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_otel_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_otel_service_name")]
    pub service_name: String,
}

impl Default for OtelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_otel_endpoint(),
            service_name: default_otel_service_name(),
        }
    }
}

fn default_otel_endpoint() -> String {
    "http://localhost:4317".into()
}

fn default_otel_service_name() -> String {
    "prism".into()
}

// --- Retry ---

#[derive(Debug, Clone, Deserialize)]
pub struct RetryConfig {
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_retry_base_delay_ms")]
    pub base_delay_ms: u64,
    #[serde(default = "default_retry_max_delay_ms")]
    pub max_delay_ms: u64,
    #[serde(default = "default_retry_jitter")]
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            base_delay_ms: default_retry_base_delay_ms(),
            max_delay_ms: default_retry_max_delay_ms(),
            jitter: default_retry_jitter(),
        }
    }
}

fn default_max_retries() -> u32 {
    3
}
fn default_retry_base_delay_ms() -> u64 {
    500
}
fn default_retry_max_delay_ms() -> u64 {
    10_000
}
fn default_retry_jitter() -> bool {
    true
}

// --- Batch ---

#[derive(Debug, Clone, Deserialize)]
pub struct BatchConfig {
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: default_max_batch_size(),
            max_concurrency: default_max_concurrency(),
        }
    }
}

fn default_max_batch_size() -> usize {
    50
}

fn default_max_concurrency() -> usize {
    10
}

// --- Waste ---

#[derive(Debug, Clone, Deserialize)]
pub struct WasteConfig {
    #[serde(default = "default_waste_enabled")]
    pub enabled: bool,
    #[serde(default = "default_quality_tolerance")]
    pub quality_tolerance: f64,
    #[serde(default = "default_cost_ratio_threshold")]
    pub cost_ratio_threshold: f64,
    #[serde(default = "default_overspend_multiplier")]
    pub overspend_multiplier: f64,
}

impl Default for WasteConfig {
    fn default() -> Self {
        Self {
            enabled: default_waste_enabled(),
            quality_tolerance: default_quality_tolerance(),
            cost_ratio_threshold: default_cost_ratio_threshold(),
            overspend_multiplier: default_overspend_multiplier(),
        }
    }
}

fn default_waste_enabled() -> bool {
    true
}
fn default_quality_tolerance() -> f64 {
    0.05
}
fn default_cost_ratio_threshold() -> f64 {
    0.5
}
fn default_overspend_multiplier() -> f64 {
    2.0
}

// --- Cache ---

#[derive(Debug, Clone, Deserialize)]
pub struct CacheConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cache_max_size")]
    pub max_size: usize,
    #[serde(default = "default_cache_ttl")]
    pub ttl_secs: u64,
    #[serde(default)]
    pub semantic: SemanticCacheConfig,
    #[serde(default = "default_cache_backend")]
    pub backend: String,
    #[serde(default)]
    pub redis_url: Option<String>,
    #[serde(default)]
    pub s3_bucket: Option<String>,
    #[serde(default)]
    pub s3_prefix: Option<String>,
}

fn default_cache_backend() -> String {
    "memory".into()
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_size: default_cache_max_size(),
            ttl_secs: default_cache_ttl(),
            semantic: SemanticCacheConfig::default(),
            backend: default_cache_backend(),
            redis_url: None,
            s3_bucket: None,
            s3_prefix: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SemanticCacheConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_semantic_max_size")]
    pub max_size: usize,
    #[serde(default = "default_semantic_ttl")]
    pub ttl_secs: u64,
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,
    #[serde(default = "default_embedding_dim")]
    pub embedding_dim: usize,
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_size: default_semantic_max_size(),
            ttl_secs: default_semantic_ttl(),
            similarity_threshold: default_similarity_threshold(),
            embedding_dim: default_embedding_dim(),
        }
    }
}

fn default_semantic_max_size() -> usize {
    500
}
fn default_semantic_ttl() -> u64 {
    7200
}
fn default_similarity_threshold() -> f32 {
    0.92
}
fn default_embedding_dim() -> usize {
    128
}

fn default_cache_max_size() -> usize {
    1000
}

fn default_cache_ttl() -> u64 {
    3600
}

// Defaults
fn default_gateway() -> GatewayConfig {
    GatewayConfig {
        address: default_address(),
    }
}

fn default_address() -> String {
    "0.0.0.0:9100".into()
}

fn default_clickhouse_url() -> String {
    "http://localhost:8123".into()
}

fn default_clickhouse_db() -> String {
    "prism".into()
}

fn default_batch_size() -> usize {
    50
}

fn default_flush_interval_ms() -> u64 {
    100
}

fn default_queue_size() -> usize {
    10_000
}

impl Default for ClickHouseConfig {
    fn default() -> Self {
        Self {
            url: default_clickhouse_url(),
            database: default_clickhouse_db(),
        }
    }
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            batch_size: default_batch_size(),
            flush_interval_ms: default_flush_interval_ms(),
            queue_size: default_queue_size(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Figment::new()
            .extract()
            .expect("Config with all defaults should always deserialize")
    }
}

impl Config {
    /// Load config from TOML file (if it exists) with PRISM_ env var overrides.
    pub fn load(path: Option<PathBuf>) -> Result<Self, figment::Error> {
        let mut figment = Figment::new();

        // Layer 1: TOML config file
        if let Some(p) = path {
            figment = figment.merge(Toml::file(p));
        } else {
            // Try default locations
            figment = figment.merge(Toml::file("prism.toml"));
            figment = figment.merge(Toml::file("config/prism.toml"));
        }

        // Layer 2: Environment variables with PRISM_ prefix
        // PRISM_GATEWAY_ADDRESS=... → gateway.address
        figment = figment.merge(Env::prefixed("PRISM_").split("_").lowercase(true));

        figment.extract()
    }
}

// --- Rate Limit ---

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    #[serde(default = "default_rate_limit_backend")]
    pub backend: String,
    #[serde(default)]
    pub redis_url: Option<String>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            backend: default_rate_limit_backend(),
            redis_url: None,
        }
    }
}

fn default_rate_limit_backend() -> String {
    "memory".into()
}

// --- Billing ---

#[derive(Debug, Clone, Deserialize)]
pub struct BillingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_discrepancy_threshold")]
    pub discrepancy_threshold_pct: f64,
}

impl Default for BillingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            discrepancy_threshold_pct: default_discrepancy_threshold(),
        }
    }
}

fn default_discrepancy_threshold() -> f64 {
    0.02
}

// --- Interop ---

#[derive(Debug, Clone, Deserialize)]
pub struct InteropConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub hmac_secret: Option<String>,
}

impl Default for InteropConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hmac_secret: None,
        }
    }
}

// --- Observability Callbacks ---

#[derive(Debug, Clone, Deserialize)]
pub struct ObservabilityCallbacksConfig {
    #[serde(default)]
    pub langfuse: Option<LangfuseConfig>,
    #[serde(default)]
    pub helicone: Option<HeliconeConfig>,
    #[serde(default)]
    pub datadog: Option<DatadogConfig>,
}

impl Default for ObservabilityCallbacksConfig {
    fn default() -> Self {
        Self {
            langfuse: None,
            helicone: None,
            datadog: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LangfuseConfig {
    pub api_url: String,
    pub public_key: String,
    pub secret_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeliconeConfig {
    pub api_key: String,
    #[serde(default = "default_helicone_url")]
    pub api_url: String,
}

fn default_helicone_url() -> String {
    "https://api.helicone.ai".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatadogConfig {
    pub api_key: String,
    #[serde(default = "default_datadog_site")]
    pub site: String,
}

fn default_datadog_site() -> String {
    "datadoghq.com".into()
}

// --- Budget Hierarchy ---

#[derive(Debug, Clone, Deserialize)]
pub struct BudgetHierarchyConfig {
    #[serde(default)]
    pub enabled: bool,
}

impl Default for BudgetHierarchyConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

// --- Dashboard ---

#[derive(Debug, Clone, Deserialize)]
pub struct DashboardConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_dashboard_dist_path")]
    pub dist_path: String,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dist_path: default_dashboard_dist_path(),
        }
    }
}

fn default_dashboard_dist_path() -> String {
    "dashboard/dist".into()
}

// --- CORS ---

#[derive(Debug, Clone, Deserialize)]
pub struct CorsConfig {
    #[serde(default = "default_cors_origins")]
    pub allowed_origins: Vec<String>,
    #[serde(default = "default_cors_methods")]
    pub allowed_methods: Vec<String>,
    #[serde(default = "default_cors_max_age_secs")]
    pub max_age_secs: u64,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: default_cors_origins(),
            allowed_methods: default_cors_methods(),
            max_age_secs: default_cors_max_age_secs(),
        }
    }
}

fn default_cors_origins() -> Vec<String> {
    vec!["*".into()]
}

fn default_cors_methods() -> Vec<String> {
    vec!["GET".into(), "POST".into(), "OPTIONS".into()]
}

fn default_cors_max_age_secs() -> u64 {
    3600
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config: Config = Figment::new().extract().unwrap();
        assert_eq!(config.gateway.address, "0.0.0.0:9100");
        assert_eq!(config.clickhouse.url, "http://localhost:8123");
        assert_eq!(config.pipeline.batch_size, 50);
    }

    #[test]
    fn test_experiment_defaults() {
        let config: Config = Figment::new().extract().unwrap();
        assert!(!config.experiments.enabled);
        assert!(config.experiments.experiments.is_empty());
    }

    #[test]
    fn test_cache_defaults() {
        let config: Config = Figment::new().extract().unwrap();
        assert!(!config.cache.enabled);
        assert_eq!(config.cache.max_size, 1000);
        assert_eq!(config.cache.ttl_secs, 3600);
    }

    #[test]
    fn test_benchmark_defaults() {
        let config: Config = Figment::new().extract().unwrap();
        assert!(!config.benchmark.enabled);
        assert!((config.benchmark.sample_rate - 0.05).abs() < f64::EPSILON);
        assert_eq!(config.benchmark.judge_model, "gpt-4o-mini");
        assert_eq!(config.benchmark.max_benchmark_models, 3);
        assert_eq!(config.benchmark.max_concurrent_benchmarks, 5);
        assert_eq!(config.benchmark.fitness_refresh_interval_secs, 300);
        assert_eq!(config.benchmark.min_sample_size, 10);
    }

    #[test]
    fn test_waste_defaults() {
        let config: Config = Figment::new().extract().unwrap();
        assert!(config.waste.enabled);
        assert!((config.waste.quality_tolerance - 0.05).abs() < f64::EPSILON);
        assert!((config.waste.cost_ratio_threshold - 0.5).abs() < f64::EPSILON);
        assert!((config.waste.overspend_multiplier - 2.0).abs() < f64::EPSILON);
    }
}
