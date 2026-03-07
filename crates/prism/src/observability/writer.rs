use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::benchmark::BenchmarkEvent;
use crate::config::ClickHouseConfig;
use crate::experiment::feedback::FeedbackEvent;
use crate::types::{EventStatus, InferenceEvent};

/// Async batch writer that drains InferenceEvents from a channel
/// and inserts them into ClickHouse in batches.
pub struct InferenceWriter {
    rx: mpsc::Receiver<InferenceEvent>,
    client: clickhouse::Client,
    batch_size: usize,
    flush_interval: Duration,
    cancel: CancellationToken,
}

impl InferenceWriter {
    pub fn new(
        rx: mpsc::Receiver<InferenceEvent>,
        ch_config: &ClickHouseConfig,
        batch_size: usize,
        flush_interval_ms: u64,
        cancel: CancellationToken,
    ) -> Self {
        let client = clickhouse::Client::default()
            .with_url(&ch_config.url)
            .with_database(&ch_config.database);

        Self {
            rx,
            client,
            batch_size,
            flush_interval: Duration::from_millis(flush_interval_ms),
            cancel,
        }
    }

    /// Apply ClickHouse schema on startup.
    pub async fn migrate(&self) -> anyhow::Result<()> {
        for schema in super::schema::ALL_SCHEMAS {
            self.client.query(schema).execute().await?;
        }
        tracing::info!("clickhouse schema applied");
        Ok(())
    }

    /// Run the writer loop. Flushes on batch_size or flush_interval, whichever first.
    pub async fn run(mut self) {
        let mut batch: Vec<InferenceEvent> = Vec::with_capacity(self.batch_size);
        let mut interval = tokio::time::interval(self.flush_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    // Drain remaining items
                    self.rx.close();
                    while let Ok(event) = self.rx.try_recv() {
                        batch.push(event);
                    }
                    if !batch.is_empty() {
                        self.flush(&mut batch).await;
                    }
                    tracing::info!("inference writer shut down");
                    return;
                }
                Some(event) = self.rx.recv() => {
                    batch.push(event);
                    if batch.len() >= self.batch_size {
                        self.flush(&mut batch).await;
                    }
                }
                _ = interval.tick() => {
                    if !batch.is_empty() {
                        self.flush(&mut batch).await;
                    }
                }
            }
        }
    }

    async fn flush(&self, batch: &mut Vec<InferenceEvent>) {
        let count = batch.len();
        if let Err(e) = self.flush_with_retry(batch).await {
            tracing::error!(error = %e, count, "failed to flush events after retries");
        }
        batch.clear();
    }

    async fn flush_with_retry(&self, batch: &[InferenceEvent]) -> anyhow::Result<()> {
        let mut last_err = None;

        for attempt in 0..3 {
            match self.insert_batch(batch).await {
                Ok(()) => {
                    tracing::debug!(count = batch.len(), "flushed events to clickhouse");
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        error = %e,
                        count = batch.len(),
                        "clickhouse insert failed, retrying"
                    );
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
                }
            }
        }

        Err(last_err.unwrap())
    }

    async fn insert_batch(&self, batch: &[InferenceEvent]) -> anyhow::Result<()> {
        let mut insert = self.client.insert("inference_events")?;

        for event in batch {
            insert.write(&ClickHouseEvent::from(event)).await?;
        }

        insert.end().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FeedbackWriter — same pattern as InferenceWriter for feedback_events
// ---------------------------------------------------------------------------

pub struct FeedbackWriter {
    rx: mpsc::Receiver<FeedbackEvent>,
    client: clickhouse::Client,
    batch_size: usize,
    flush_interval: Duration,
    cancel: CancellationToken,
}

impl FeedbackWriter {
    pub fn new(
        rx: mpsc::Receiver<FeedbackEvent>,
        ch_config: &ClickHouseConfig,
        batch_size: usize,
        flush_interval_ms: u64,
        cancel: CancellationToken,
    ) -> Self {
        let client = clickhouse::Client::default()
            .with_url(&ch_config.url)
            .with_database(&ch_config.database);

        Self {
            rx,
            client,
            batch_size,
            flush_interval: Duration::from_millis(flush_interval_ms),
            cancel,
        }
    }

    pub async fn run(mut self) {
        let mut batch: Vec<FeedbackEvent> = Vec::with_capacity(self.batch_size);
        let mut interval = tokio::time::interval(self.flush_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    self.rx.close();
                    while let Ok(event) = self.rx.try_recv() {
                        batch.push(event);
                    }
                    if !batch.is_empty() {
                        self.flush(&mut batch).await;
                    }
                    tracing::info!("feedback writer shut down");
                    return;
                }
                Some(event) = self.rx.recv() => {
                    batch.push(event);
                    if batch.len() >= self.batch_size {
                        self.flush(&mut batch).await;
                    }
                }
                _ = interval.tick() => {
                    if !batch.is_empty() {
                        self.flush(&mut batch).await;
                    }
                }
            }
        }
    }

    async fn flush(&self, batch: &mut Vec<FeedbackEvent>) {
        let count = batch.len();
        if let Err(e) = self.flush_with_retry(batch).await {
            tracing::error!(error = %e, count, "failed to flush feedback events after retries");
        }
        batch.clear();
    }

    async fn flush_with_retry(&self, batch: &[FeedbackEvent]) -> anyhow::Result<()> {
        let mut last_err = None;

        for attempt in 0..3 {
            match self.insert_batch(batch).await {
                Ok(()) => {
                    tracing::debug!(count = batch.len(), "flushed feedback events to clickhouse");
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        error = %e,
                        count = batch.len(),
                        "clickhouse feedback insert failed, retrying"
                    );
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
                }
            }
        }

        Err(last_err.unwrap())
    }

    async fn insert_batch(&self, batch: &[FeedbackEvent]) -> anyhow::Result<()> {
        let mut insert = self.client.insert("feedback_events")?;

        for event in batch {
            insert.write(&ClickHouseFeedbackEvent::from(event)).await?;
        }

        insert.end().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// BenchmarkWriter — same pattern as FeedbackWriter for benchmark_events
// ---------------------------------------------------------------------------

pub struct BenchmarkWriter {
    rx: mpsc::Receiver<BenchmarkEvent>,
    client: clickhouse::Client,
    batch_size: usize,
    flush_interval: Duration,
    cancel: CancellationToken,
}

impl BenchmarkWriter {
    pub fn new(
        rx: mpsc::Receiver<BenchmarkEvent>,
        ch_config: &ClickHouseConfig,
        batch_size: usize,
        flush_interval_ms: u64,
        cancel: CancellationToken,
    ) -> Self {
        let client = clickhouse::Client::default()
            .with_url(&ch_config.url)
            .with_database(&ch_config.database);

        Self {
            rx,
            client,
            batch_size,
            flush_interval: Duration::from_millis(flush_interval_ms),
            cancel,
        }
    }

    pub async fn run(mut self) {
        let mut batch: Vec<BenchmarkEvent> = Vec::with_capacity(self.batch_size);
        let mut interval = tokio::time::interval(self.flush_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    self.rx.close();
                    while let Ok(event) = self.rx.try_recv() {
                        batch.push(event);
                    }
                    if !batch.is_empty() {
                        self.flush(&mut batch).await;
                    }
                    tracing::info!("benchmark writer shut down");
                    return;
                }
                Some(event) = self.rx.recv() => {
                    batch.push(event);
                    if batch.len() >= self.batch_size {
                        self.flush(&mut batch).await;
                    }
                }
                _ = interval.tick() => {
                    if !batch.is_empty() {
                        self.flush(&mut batch).await;
                    }
                }
            }
        }
    }

    async fn flush(&self, batch: &mut Vec<BenchmarkEvent>) {
        let count = batch.len();
        if let Err(e) = self.flush_with_retry(batch).await {
            tracing::error!(error = %e, count, "failed to flush benchmark events after retries");
        }
        batch.clear();
    }

    async fn flush_with_retry(&self, batch: &[BenchmarkEvent]) -> anyhow::Result<()> {
        let mut last_err = None;

        for attempt in 0..3 {
            match self.insert_batch(batch).await {
                Ok(()) => {
                    tracing::debug!(
                        count = batch.len(),
                        "flushed benchmark events to clickhouse"
                    );
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        error = %e,
                        count = batch.len(),
                        "clickhouse benchmark insert failed, retrying"
                    );
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
                }
            }
        }

        Err(last_err.unwrap())
    }

    async fn insert_batch(&self, batch: &[BenchmarkEvent]) -> anyhow::Result<()> {
        let mut insert = self.client.insert("benchmark_events")?;

        for event in batch {
            insert.write(&ClickHouseBenchmarkEvent::from(event)).await?;
        }

        insert.end().await?;
        Ok(())
    }
}

/// Row type for ClickHouse benchmark_events table.
#[derive(Debug, clickhouse::Row, serde::Serialize)]
struct ClickHouseBenchmarkEvent {
    id: uuid::Uuid,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    timestamp: time::OffsetDateTime,
    inference_id: uuid::Uuid,
    task_type: Option<String>,
    original_model: String,
    benchmark_model: String,
    judge_model: String,
    original_score: f64,
    benchmark_score: f64,
    benchmark_cost: f64,
    benchmark_latency_ms: u32,
    judge_cost: f64,
    prompt_hash: String,
    status: String,
}

impl From<&BenchmarkEvent> for ClickHouseBenchmarkEvent {
    fn from(e: &BenchmarkEvent) -> Self {
        Self {
            id: e.id,
            timestamp: time::OffsetDateTime::from_unix_timestamp(e.timestamp.timestamp())
                .unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
            inference_id: e.inference_id,
            task_type: e.task_type.map(|t| t.to_string()),
            original_model: e.original_model.clone(),
            benchmark_model: e.benchmark_model.clone(),
            judge_model: e.judge_model.clone(),
            original_score: e.original_score,
            benchmark_score: e.benchmark_score,
            benchmark_cost: e.benchmark_cost,
            benchmark_latency_ms: e.benchmark_latency_ms,
            judge_cost: e.judge_cost,
            prompt_hash: e.prompt_hash.clone(),
            status: e.status.clone(),
        }
    }
}

/// Row type for ClickHouse feedback_events table.
#[derive(Debug, clickhouse::Row, serde::Serialize)]
struct ClickHouseFeedbackEvent {
    id: uuid::Uuid,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    timestamp: time::OffsetDateTime,
    inference_id: Option<uuid::Uuid>,
    episode_id: Option<uuid::Uuid>,
    metric_name: String,
    metric_value: f64,
    metadata: String,
}

impl From<&FeedbackEvent> for ClickHouseFeedbackEvent {
    fn from(e: &FeedbackEvent) -> Self {
        Self {
            id: e.id,
            timestamp: time::OffsetDateTime::from_unix_timestamp(e.timestamp.timestamp())
                .unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
            inference_id: e.inference_id,
            episode_id: e.episode_id,
            metric_name: e.metric_name.clone(),
            metric_value: e.metric_value,
            metadata: e.metadata.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// ClickHouseEvent — inference_events row type
// ---------------------------------------------------------------------------

/// Row type for ClickHouse insertion, matching the table schema.
#[derive(Debug, clickhouse::Row, serde::Serialize)]
struct ClickHouseEvent {
    id: uuid::Uuid,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    timestamp: time::OffsetDateTime,
    provider: String,
    model: String,
    status: String,
    input_tokens: u32,
    output_tokens: u32,
    total_tokens: u32,
    cache_read_input_tokens: u32,
    cache_creation_input_tokens: u32,
    estimated_cost_usd: f64,
    latency_ms: u32,
    prompt_hash: String,
    completion_hash: String,
    task_type: Option<String>,
    routing_decision: Option<String>,
    variant_name: Option<String>,
    virtual_key_hash: Option<String>,
    team_id: Option<String>,
    end_user_id: Option<String>,
    episode_id: Option<uuid::Uuid>,
    metadata: String,
    trace_id: Option<String>,
    span_id: Option<String>,
    parent_span_id: Option<String>,
    agent_framework: Option<String>,
    tool_calls_json: Option<String>,
    ttft_ms: Option<u32>,
    session_id: Option<String>,
    provider_attempted: Option<String>,
}

impl From<&InferenceEvent> for ClickHouseEvent {
    fn from(e: &InferenceEvent) -> Self {
        Self {
            id: e.id,
            timestamp: time::OffsetDateTime::from_unix_timestamp(e.timestamp.timestamp())
                .unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
            provider: e.provider.clone(),
            model: e.model.clone(),
            status: match e.status {
                EventStatus::Success => "success".into(),
                EventStatus::Failure => "failure".into(),
            },
            input_tokens: e.input_tokens,
            output_tokens: e.output_tokens,
            total_tokens: e.total_tokens,
            cache_read_input_tokens: e.cache_read_input_tokens,
            cache_creation_input_tokens: e.cache_creation_input_tokens,
            estimated_cost_usd: e.estimated_cost_usd,
            latency_ms: e.latency_ms,
            prompt_hash: e.prompt_hash.clone(),
            completion_hash: e.completion_hash.clone(),
            task_type: e.task_type.map(|t| t.to_string()),
            routing_decision: e.routing_decision.clone(),
            variant_name: e.variant_name.clone(),
            virtual_key_hash: e.virtual_key_hash.clone(),
            team_id: e.team_id.clone(),
            end_user_id: e.end_user_id.clone(),
            episode_id: e.episode_id,
            metadata: e.metadata.clone(),
            trace_id: e.trace_id.clone(),
            span_id: e.span_id.clone(),
            parent_span_id: e.parent_span_id.clone(),
            agent_framework: e.agent_framework.clone(),
            tool_calls_json: e.tool_calls_json.clone(),
            ttft_ms: e.ttft_ms,
            session_id: e.session_id.clone(),
            provider_attempted: e.provider_attempted.clone(),
        }
    }
}
