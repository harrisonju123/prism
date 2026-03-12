use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::benchmark::judge::Judge;
use crate::config::Config;
use crate::providers::ProviderRegistry;
use crate::routing::FitnessCache;
use crate::types::{Message, MessageRole, TaskType};

/// A sampled completion row read from ClickHouse completion_samples.
struct LiveSample {
    id: Uuid,
    model: String,
    task_type: Option<TaskType>,
    prompt_messages: String,
    completion_text: String,
    input_tokens: u32,
    output_tokens: u32,
    latency_ms: u64,
    cost_usd: f64,
}

/// Background task that samples unjudged completions from ClickHouse,
/// runs the LLM-judge on each, and updates the fitness cache.
///
/// Rate-limited to at most `max_calls_per_minute` judge invocations.
pub struct LiveJudgeTask {
    providers: Arc<ProviderRegistry>,
    config: Config,
    fitness_cache: FitnessCache,
    cancel: CancellationToken,
    interval_secs: u64,
    max_calls_per_minute: u32,
    lookback_secs: u64,
    ch_url: String,
    ch_db: String,
    client: reqwest::Client,
}

impl LiveJudgeTask {
    pub fn new(
        providers: Arc<ProviderRegistry>,
        config: Config,
        fitness_cache: FitnessCache,
        cancel: CancellationToken,
        interval_secs: u64,
        max_calls_per_minute: u32,
        lookback_secs: u64,
        ch_url: String,
        ch_db: String,
    ) -> Self {
        Self {
            providers,
            config,
            fitness_cache,
            cancel,
            interval_secs,
            max_calls_per_minute,
            lookback_secs,
            ch_url,
            ch_db,
            client: reqwest::Client::new(),
        }
    }

    pub async fn run(self) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(self.interval_secs)) => {
                    if let Err(e) = self.run_once().await {
                        tracing::warn!(error = %e, "live judge run failed");
                    }
                }
                _ = self.cancel.cancelled() => {
                    tracing::debug!("live judge task shut down");
                    return;
                }
            }
        }
    }

    async fn run_once(&self) -> anyhow::Result<()> {
        let samples = self.fetch_samples().await?;

        if samples.is_empty() {
            tracing::debug!("no live samples available for judge");
            return Ok(());
        }

        tracing::info!(
            count = samples.len(),
            max_calls_per_minute = self.max_calls_per_minute,
            "running live judge on {} samples",
            samples.len()
        );

        let judge = Judge::new(self.config.benchmark.judge_model.clone());

        let allowed = (self.max_calls_per_minute as usize).min(samples.len());
        let min_gap_ms = if allowed > 0 {
            60_000 / allowed as u64
        } else {
            0
        };

        let mut last_call = Instant::now()
            .checked_sub(Duration::from_secs(60))
            .unwrap_or_else(Instant::now);
        let mut judged_ids: Vec<Uuid> = Vec::new();
        let mut judged = 0;

        for sample in samples.iter().take(allowed) {
            if self.cancel.is_cancelled() {
                break;
            }

            let elapsed = last_call.elapsed();
            let gap = Duration::from_millis(min_gap_ms);
            if elapsed < gap {
                tokio::time::sleep(gap - elapsed).await;
            }

            let score = self.judge_sample(&judge, sample).await;

            if let (Some(task_type), Some(quality)) = (sample.task_type, score) {
                let total_tokens = sample.input_tokens + sample.output_tokens;
                let cost_per_1k = if total_tokens > 0 {
                    sample.cost_usd / (total_tokens as f64 / 1000.0)
                } else {
                    0.0
                };

                self.fitness_cache
                    .record_benchmark(
                        task_type,
                        &sample.model,
                        quality,
                        cost_per_1k,
                        sample.latency_ms as f64,
                    )
                    .await;

                tracing::debug!(
                    model = %sample.model,
                    task_type = ?task_type,
                    quality,
                    "live judge updated fitness cache"
                );
                judged_ids.push(sample.id);
                judged += 1;
            }

            last_call = Instant::now();
        }

        if !judged_ids.is_empty() {
            if let Err(e) = self.mark_judged(&judged_ids).await {
                tracing::warn!(error = %e, count = judged_ids.len(), "failed to mark samples as judged");
            }
        }

        tracing::info!(judged, samples = samples.len(), "live judge cycle complete");
        Ok(())
    }

    async fn fetch_samples(&self) -> anyhow::Result<Vec<LiveSample>> {
        let query = format!(
            "SELECT id, model, task_type, \
                    prompt_messages, completion_text, \
                    input_tokens, output_tokens, \
                    latency_ms, estimated_cost_usd AS cost_usd \
             FROM {db}.completion_samples \
             WHERE judged = 0 \
               AND task_type IS NOT NULL \
               AND timestamp >= now() - INTERVAL {secs} SECOND \
             ORDER BY rand() \
             LIMIT {limit} \
             FORMAT JSONEachRow",
            db = self.ch_db,
            secs = self.lookback_secs,
            limit = self.max_calls_per_minute * 2,
        );

        let resp = self
            .client
            .post(&self.ch_url)
            .body(query)
            .send()
            .await?
            .text()
            .await?;

        let mut samples = Vec::new();
        for line in resp.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                let id = match v
                    .get("id")
                    .and_then(|x| x.as_str())
                    .and_then(|s| s.parse::<Uuid>().ok())
                {
                    Some(id) => id,
                    None => continue,
                };
                let model = match v.get("model").and_then(|x| x.as_str()) {
                    Some(m) => m.to_string(),
                    None => continue,
                };
                let task_type_str = v.get("task_type").and_then(|x| x.as_str()).unwrap_or("");
                let task_type = serde_json::from_value::<TaskType>(serde_json::Value::String(
                    task_type_str.to_string(),
                ))
                .ok();
                let prompt_messages = v
                    .get("prompt_messages")
                    .and_then(|x| x.as_str())
                    .unwrap_or("[]")
                    .to_string();
                let completion_text = v
                    .get("completion_text")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let input_tokens =
                    v.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let output_tokens =
                    v.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let latency_ms = v.get("latency_ms").and_then(|x| x.as_f64()).unwrap_or(0.0) as u64;
                let cost_usd = v.get("cost_usd").and_then(|x| x.as_f64()).unwrap_or(0.0);

                if !completion_text.is_empty() {
                    samples.push(LiveSample {
                        id,
                        model,
                        task_type,
                        prompt_messages,
                        completion_text,
                        input_tokens,
                        output_tokens,
                        latency_ms,
                        cost_usd,
                    });
                }
            }
        }
        Ok(samples)
    }

    async fn judge_sample(&self, judge: &Judge, sample: &LiveSample) -> Option<f64> {
        // Deserialize the stored prompt messages back to Vec<Message>
        let messages: Vec<Message> =
            serde_json::from_str(&sample.prompt_messages).unwrap_or_else(|_| {
                // Fallback: single synthetic user message if deserialization fails
                vec![Message {
                    role: MessageRole::User,
                    content: Some(serde_json::Value::String(format!(
                        "sample_id: {}",
                        sample.id
                    ))),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: serde_json::Map::new(),
                }]
            });

        match judge
            .score_absolute(
                &self.providers,
                &self.config,
                sample.task_type,
                &messages,
                &sample.completion_text,
            )
            .await
        {
            Ok(score) => Some(score),
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    model = %sample.model,
                    "live judge absolute score failed for sample"
                );
                None
            }
        }
    }

    async fn mark_judged(&self, ids: &[Uuid]) -> anyhow::Result<()> {
        let id_list = ids
            .iter()
            .map(|id| format!("'{id}'"))
            .collect::<Vec<_>>()
            .join(", ");

        let query = format!(
            "ALTER TABLE {db}.completion_samples UPDATE judged = 1 WHERE id IN ({ids})",
            db = self.ch_db,
            ids = id_list,
        );

        self.client
            .post(&self.ch_url)
            .body(query)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_sample_cost_per_1k_calculation() {
        // 1000 tokens, cost $1.00 → $1.00/1k
        let total_tokens: u32 = 1000;
        let cost_usd: f64 = 1.0;
        let cost_per_1k = cost_usd / (total_tokens as f64 / 1000.0);
        assert!((cost_per_1k - 1.0).abs() < 1e-9);

        // 500 tokens, cost $0.001 → $0.002/1k
        let total_tokens: u32 = 500;
        let cost_usd: f64 = 0.001;
        let cost_per_1k = cost_usd / (total_tokens as f64 / 1000.0);
        assert!((cost_per_1k - 0.002).abs() < 1e-9);
    }

    #[test]
    fn live_sample_cost_per_1k_zero_tokens() {
        let total_tokens: u32 = 0;
        let cost_usd: f64 = 0.001;
        let cost_per_1k = if total_tokens > 0 {
            cost_usd / (total_tokens as f64 / 1000.0)
        } else {
            0.0
        };
        assert_eq!(cost_per_1k, 0.0);
    }

    #[test]
    fn prompt_messages_json_roundtrip() {
        use crate::types::MessageRole;
        let messages = vec![Message {
            role: MessageRole::User,
            content: Some(serde_json::Value::String("hello".into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: serde_json::Map::new(),
        }];
        let json = serde_json::to_string(&messages).unwrap();
        let parsed: Vec<Message> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].role, MessageRole::User);
    }
}
