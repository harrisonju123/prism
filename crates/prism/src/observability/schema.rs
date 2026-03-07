/// ClickHouse schema for PrisM. Applied on startup.
pub const INFERENCE_EVENTS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS inference_events (
    id UUID,
    timestamp DateTime64(3),
    provider LowCardinality(String),
    model LowCardinality(String),
    status Enum8('success' = 1, 'failure' = 2),
    input_tokens UInt32,
    output_tokens UInt32,
    total_tokens UInt32,
    cache_read_input_tokens UInt32 DEFAULT 0,
    cache_creation_input_tokens UInt32 DEFAULT 0,
    estimated_cost_usd Float64,
    latency_ms UInt32,
    prompt_hash String,
    completion_hash String,
    task_type LowCardinality(Nullable(String)),
    routing_decision Nullable(String),
    variant_name Nullable(String),
    virtual_key_hash Nullable(String),
    team_id Nullable(String),
    end_user_id Nullable(String),
    episode_id Nullable(UUID),
    metadata String DEFAULT '{}',
    trace_id Nullable(String),
    span_id Nullable(String),
    parent_span_id Nullable(String),
    agent_framework LowCardinality(Nullable(String)),
    tool_calls_json Nullable(String),
    ttft_ms Nullable(UInt32),
    session_id Nullable(String),
    provider_attempted Nullable(String)
) ENGINE = MergeTree()
PARTITION BY toYYYYMM(timestamp)
ORDER BY (timestamp, provider, model)
TTL timestamp + INTERVAL 90 DAY
"#;

/// Materialized view for hourly model stats.
pub const HOURLY_MODEL_STATS_SCHEMA: &str = r#"
CREATE MATERIALIZED VIEW IF NOT EXISTS hourly_model_stats
ENGINE = SummingMergeTree()
PARTITION BY toYYYYMM(hour)
ORDER BY (hour, provider, model)
AS SELECT
    toStartOfHour(timestamp) AS hour,
    provider,
    model,
    count() AS request_count,
    sum(input_tokens) AS total_input_tokens,
    sum(output_tokens) AS total_output_tokens,
    sum(estimated_cost_usd) AS total_cost_usd,
    avg(latency_ms) AS avg_latency_ms,
    quantile(0.99)(latency_ms) AS p99_latency_ms
FROM inference_events
GROUP BY hour, provider, model
"#;

/// Materialized view for daily summary.
pub const DAILY_SUMMARY_SCHEMA: &str = r#"
CREATE MATERIALIZED VIEW IF NOT EXISTS daily_summary
ENGINE = SummingMergeTree()
PARTITION BY toYYYYMM(day)
ORDER BY (day)
AS SELECT
    toStartOfDay(timestamp) AS day,
    count() AS request_count,
    sum(input_tokens) AS total_input_tokens,
    sum(output_tokens) AS total_output_tokens,
    sum(estimated_cost_usd) AS total_cost_usd,
    avg(latency_ms) AS avg_latency_ms
FROM inference_events
GROUP BY day
"#;

/// ClickHouse table for feedback events.
pub const FEEDBACK_EVENTS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS feedback_events (
    id UUID,
    timestamp DateTime64(3),
    inference_id Nullable(UUID),
    episode_id Nullable(UUID),
    metric_name LowCardinality(String),
    metric_value Float64,
    metadata String DEFAULT '{}'
) ENGINE = MergeTree()
PARTITION BY toYYYYMM(timestamp)
ORDER BY (timestamp, metric_name)
TTL timestamp + INTERVAL 90 DAY
"#;

/// ClickHouse table for benchmark events.
pub const BENCHMARK_EVENTS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS benchmark_events (
    id UUID,
    timestamp DateTime64(3),
    inference_id UUID,
    task_type LowCardinality(Nullable(String)),
    original_model LowCardinality(String),
    benchmark_model LowCardinality(String),
    judge_model LowCardinality(String),
    original_score Float64,
    benchmark_score Float64,
    benchmark_cost Float64,
    benchmark_latency_ms UInt32,
    judge_cost Float64,
    prompt_hash String,
    status LowCardinality(String)
) ENGINE = MergeTree()
PARTITION BY toYYYYMM(timestamp)
ORDER BY (timestamp, task_type, benchmark_model)
TTL timestamp + INTERVAL 90 DAY
"#;

/// ALTER TABLE migrations for existing inference_events tables.
/// ClickHouse ignores ADD COLUMN IF NOT EXISTS if column already exists.
pub const INFERENCE_EVENTS_MIGRATION_V2: &str = r#"
ALTER TABLE inference_events
    ADD COLUMN IF NOT EXISTS cache_read_input_tokens UInt32 DEFAULT 0,
    ADD COLUMN IF NOT EXISTS cache_creation_input_tokens UInt32 DEFAULT 0,
    ADD COLUMN IF NOT EXISTS trace_id Nullable(String),
    ADD COLUMN IF NOT EXISTS span_id Nullable(String),
    ADD COLUMN IF NOT EXISTS parent_span_id Nullable(String),
    ADD COLUMN IF NOT EXISTS agent_framework LowCardinality(Nullable(String)),
    ADD COLUMN IF NOT EXISTS tool_calls_json Nullable(String),
    ADD COLUMN IF NOT EXISTS ttft_ms Nullable(UInt32),
    ADD COLUMN IF NOT EXISTS end_user_id Nullable(String),
    ADD COLUMN IF NOT EXISTS session_id Nullable(String),
    ADD COLUMN IF NOT EXISTS provider_attempted Nullable(String)
"#;

/// ClickHouse table for MCP tool call events.
pub const MCP_CALLS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS mcp_calls (
    id UUID,
    timestamp DateTime64(3),
    trace_id String,
    span_id Nullable(String),
    parent_span_id Nullable(String),
    server LowCardinality(String),
    method LowCardinality(String),
    tool_name String,
    args_hash String,
    inference_id UUID,
    model LowCardinality(String),
    estimated_cost Float64
) ENGINE = MergeTree()
PARTITION BY toYYYYMM(timestamp)
ORDER BY (timestamp, server, method)
TTL timestamp + INTERVAL 90 DAY
"#;

/// Materialized view for hourly task type stats.
pub const HOURLY_TASK_STATS_SCHEMA: &str = r#"
CREATE MATERIALIZED VIEW IF NOT EXISTS hourly_task_stats
ENGINE = SummingMergeTree()
PARTITION BY toYYYYMM(hour)
ORDER BY (hour, task_type)
AS SELECT
    toStartOfHour(timestamp) AS hour,
    task_type,
    count() AS request_count,
    sum(estimated_cost_usd) AS total_cost_usd,
    avg(latency_ms) AS avg_latency_ms,
    quantile(0.95)(latency_ms) AS p95_latency_ms
FROM inference_events
WHERE task_type IS NOT NULL
GROUP BY hour, task_type
"#;

/// Materialized view for daily spend by virtual key.
pub const DAILY_SPEND_BY_KEY_SCHEMA: &str = r#"
CREATE MATERIALIZED VIEW IF NOT EXISTS daily_spend_by_key
ENGINE = SummingMergeTree()
PARTITION BY toYYYYMM(day)
ORDER BY (day, virtual_key_hash)
AS SELECT
    toStartOfDay(timestamp) AS day,
    virtual_key_hash,
    count() AS request_count,
    sum(estimated_cost_usd) AS total_cost_usd,
    sum(input_tokens) AS total_input_tokens,
    sum(output_tokens) AS total_output_tokens
FROM inference_events
WHERE virtual_key_hash IS NOT NULL AND virtual_key_hash != ''
GROUP BY day, virtual_key_hash
"#;

/// Materialized view for hourly error rate by model.
pub const HOURLY_ERROR_RATE_SCHEMA: &str = r#"
CREATE MATERIALIZED VIEW IF NOT EXISTS hourly_error_rate
ENGINE = SummingMergeTree()
PARTITION BY toYYYYMM(hour)
ORDER BY (hour, model)
AS SELECT
    toStartOfHour(timestamp) AS hour,
    model,
    count() AS total_requests,
    countIf(status = 'success') AS success_count,
    countIf(status = 'failure') AS failure_count
FROM inference_events
GROUP BY hour, model
"#;

/// All schemas to apply in order.
pub const ALL_SCHEMAS: &[&str] = &[
    INFERENCE_EVENTS_SCHEMA,
    INFERENCE_EVENTS_MIGRATION_V2,
    HOURLY_MODEL_STATS_SCHEMA,
    DAILY_SUMMARY_SCHEMA,
    FEEDBACK_EVENTS_SCHEMA,
    BENCHMARK_EVENTS_SCHEMA,
    MCP_CALLS_SCHEMA,
    HOURLY_TASK_STATS_SCHEMA,
    DAILY_SPEND_BY_KEY_SCHEMA,
    HOURLY_ERROR_RATE_SCHEMA,
];
