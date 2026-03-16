use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::types::InferenceEvent;

/// SQLite-backed inference event writer for embedded/local gateway mode.
///
/// Stores events in `.prism/observability.db` for waste detection and stats
/// queries without requiring ClickHouse.
pub struct LocalInferenceWriter {
    pool: SqlitePool,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS inference_events (
    id                      TEXT PRIMARY KEY,
    timestamp               TEXT NOT NULL,
    provider                TEXT NOT NULL,
    model                   TEXT NOT NULL,
    status                  TEXT NOT NULL,
    input_tokens            INTEGER NOT NULL DEFAULT 0,
    output_tokens           INTEGER NOT NULL DEFAULT 0,
    total_tokens            INTEGER NOT NULL DEFAULT 0,
    cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
    estimated_cost_usd      REAL    NOT NULL DEFAULT 0.0,
    latency_ms              INTEGER NOT NULL DEFAULT 0,
    prompt_hash             TEXT    NOT NULL DEFAULT '',
    completion_hash         TEXT    NOT NULL DEFAULT '',
    task_type               TEXT,
    trace_id                TEXT,
    tool_calls_json         TEXT
);
CREATE INDEX IF NOT EXISTS idx_ie_timestamp    ON inference_events(timestamp);
CREATE INDEX IF NOT EXISTS idx_ie_model        ON inference_events(model);
CREATE INDEX IF NOT EXISTS idx_ie_prompt_hash  ON inference_events(prompt_hash);
CREATE INDEX IF NOT EXISTS idx_ie_trace_id     ON inference_events(trace_id);
";

impl LocalInferenceWriter {
    /// Open (or create) the SQLite database at `path`.
    pub async fn open(path: &Path) -> anyhow::Result<Arc<Self>> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let url = format!("sqlite://{}?mode=rwc", path.display());
        // Single connection: SQLite WAL mode serialises writers, extra connections waste FDs.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await?;

        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA synchronous=NORMAL")
            .execute(&pool)
            .await?;

        // Apply schema (idempotent)
        for stmt in SCHEMA.split(';') {
            let stmt = stmt.trim();
            if !stmt.is_empty() {
                sqlx::query(stmt).execute(&pool).await?;
            }
        }

        Ok(Arc::new(Self { pool }))
    }

    /// Insert a batch of inference events (best-effort — ignores duplicates).
    pub async fn insert_batch(&self, events: &[InferenceEvent]) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for e in events {
            sqlx::query(
                "INSERT OR IGNORE INTO inference_events \
                 (id, timestamp, provider, model, status, \
                  input_tokens, output_tokens, total_tokens, cache_read_input_tokens, \
                  estimated_cost_usd, latency_ms, prompt_hash, completion_hash, \
                  task_type, trace_id, tool_calls_json) \
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            )
            .bind(e.id.to_string())
            .bind(e.timestamp.to_rfc3339())
            .bind(&e.provider)
            .bind(&e.model)
            .bind(format!("{:?}", e.status))
            .bind(e.input_tokens as i64)
            .bind(e.output_tokens as i64)
            .bind(e.total_tokens as i64)
            .bind(e.cache_read_input_tokens as i64)
            .bind(e.estimated_cost_usd)
            .bind(e.latency_ms as i64)
            .bind(&e.prompt_hash)
            .bind(&e.completion_hash)
            .bind(e.task_type.as_ref().map(|t| format!("{:?}", t)))
            .bind(&e.trace_id)
            .bind(&e.tool_calls_json)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Delete events older than `cutoff`. Returns rows deleted.
    pub async fn prune_before(&self, cutoff: DateTime<Utc>) -> anyhow::Result<u64> {
        let result = sqlx::query("DELETE FROM inference_events WHERE timestamp < ?")
            .bind(cutoff.to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// Spawn the background task that drains `event_rx`, feeds `metrics`, and
/// flushes batches to `writer`.  Returns a `JoinHandle` that shuts down when
/// `cancel` is cancelled.
pub fn spawn_event_consumer(
    mut event_rx: mpsc::Receiver<InferenceEvent>,
    metrics: Arc<crate::observability::metrics::MetricsCollector>,
    writer: Option<Arc<LocalInferenceWriter>>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    const BATCH_SIZE: usize = 50;
    const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

    tokio::spawn(async move {
        let mut batch: Vec<InferenceEvent> = Vec::with_capacity(BATCH_SIZE);
        let mut interval = tokio::time::interval(FLUSH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;

                _ = cancel.cancelled() => {
                    // Drain remaining events before exit
                    while let Ok(ev) = event_rx.try_recv() {
                        record_metrics(&metrics, &ev);
                        batch.push(ev);
                    }
                    flush(&writer, &mut batch).await;
                    break;
                }

                Some(ev) = event_rx.recv() => {
                    record_metrics(&metrics, &ev);
                    batch.push(ev);
                    if batch.len() >= BATCH_SIZE {
                        flush(&writer, &mut batch).await;
                    }
                }

                _ = interval.tick() => {
                    if !batch.is_empty() {
                        flush(&writer, &mut batch).await;
                    }
                }
            }
        }
    })
}

fn record_metrics(metrics: &crate::observability::metrics::MetricsCollector, ev: &InferenceEvent) {
    use crate::types::EventStatus;
    let is_error = ev.status == EventStatus::Failure;
    metrics.record_request(&ev.model, ev.latency_ms as u64, is_error);
    metrics.record_tokens((ev.input_tokens + ev.output_tokens) as u64);
    metrics.record_cost(ev.estimated_cost_usd);
    if ev.cache_read_input_tokens > 0 {
        metrics.record_cache_hit();
    }
}

async fn flush(writer: &Option<Arc<LocalInferenceWriter>>, batch: &mut Vec<InferenceEvent>) {
    if batch.is_empty() {
        return;
    }
    if let Some(w) = writer {
        if let Err(e) = w.insert_batch(batch).await {
            tracing::warn!(error = %e, "local_writer flush failed");
        }
    }
    batch.clear();
}
