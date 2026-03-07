use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::ClickHouseConfig;
use crate::mcp::types::McpCall;

/// Async batch writer for MCP tool call events → ClickHouse.
pub struct McpWriter {
    rx: mpsc::Receiver<McpCall>,
    client: clickhouse::Client,
    batch_size: usize,
    flush_interval: Duration,
    cancel: CancellationToken,
}

impl McpWriter {
    pub fn new(
        rx: mpsc::Receiver<McpCall>,
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
        let mut batch: Vec<McpCall> = Vec::with_capacity(self.batch_size);
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
                    tracing::info!("mcp writer shut down");
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

    async fn flush(&self, batch: &mut Vec<McpCall>) {
        let count = batch.len();
        if let Err(e) = self.flush_with_retry(batch).await {
            tracing::error!(error = %e, count, "failed to flush mcp events after retries");
        }
        batch.clear();
    }

    async fn flush_with_retry(&self, batch: &[McpCall]) -> anyhow::Result<()> {
        let mut last_err = None;

        for attempt in 0..3 {
            match self.insert_batch(batch).await {
                Ok(()) => {
                    tracing::debug!(count = batch.len(), "flushed mcp events to clickhouse");
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        error = %e,
                        count = batch.len(),
                        "clickhouse mcp insert failed, retrying"
                    );
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
                }
            }
        }

        Err(last_err.unwrap())
    }

    async fn insert_batch(&self, batch: &[McpCall]) -> anyhow::Result<()> {
        let mut insert = self.client.insert("mcp_calls")?;

        for event in batch {
            insert.write(&ClickHouseMcpCall::from(event)).await?;
        }

        insert.end().await?;
        Ok(())
    }
}

/// Row type for ClickHouse mcp_calls table.
#[derive(Debug, clickhouse::Row, serde::Serialize)]
struct ClickHouseMcpCall {
    id: uuid::Uuid,
    #[serde(with = "clickhouse::serde::time::datetime64::millis")]
    timestamp: time::OffsetDateTime,
    trace_id: String,
    span_id: Option<String>,
    parent_span_id: Option<String>,
    server: String,
    method: String,
    tool_name: String,
    args_hash: String,
    inference_id: uuid::Uuid,
    model: String,
    estimated_cost: f64,
}

impl From<&McpCall> for ClickHouseMcpCall {
    fn from(e: &McpCall) -> Self {
        Self {
            id: e.id,
            timestamp: time::OffsetDateTime::from_unix_timestamp(e.timestamp.timestamp())
                .unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
            trace_id: e.trace_id.clone(),
            span_id: e.span_id.clone(),
            parent_span_id: e.parent_span_id.clone(),
            server: e.server.clone(),
            method: e.method.clone(),
            tool_name: e.tool_name.clone(),
            args_hash: e.args_hash.clone(),
            inference_id: e.inference_id,
            model: e.model.clone(),
            estimated_cost: e.estimated_cost,
        }
    }
}
