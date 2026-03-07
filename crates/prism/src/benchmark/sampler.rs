use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::benchmark::judge::Judge;
use crate::benchmark::{BenchmarkEvent, BenchmarkRequest};
use crate::config::Config;
use crate::models::MODEL_CATALOG;
use crate::providers::ProviderRegistry;
use crate::proxy::cost::compute_cost;
use crate::routing::FitnessCache;

pub struct BenchmarkSampler {
    rx: mpsc::Receiver<BenchmarkRequest>,
    result_tx: mpsc::Sender<BenchmarkEvent>,
    config: Config,
    providers: Arc<ProviderRegistry>,
    judge: Arc<Judge>,
    cancel: CancellationToken,
    fitness_cache: FitnessCache,
}

impl BenchmarkSampler {
    pub fn new(
        rx: mpsc::Receiver<BenchmarkRequest>,
        result_tx: mpsc::Sender<BenchmarkEvent>,
        config: Config,
        providers: Arc<ProviderRegistry>,
        judge: Judge,
        cancel: CancellationToken,
        fitness_cache: FitnessCache,
    ) -> Self {
        Self {
            rx,
            result_tx,
            config,
            providers,
            judge: Arc::new(judge),
            cancel,
            fitness_cache,
        }
    }

    pub async fn run(mut self) {
        let sem = Arc::new(Semaphore::new(
            self.config.benchmark.max_concurrent_benchmarks,
        ));

        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    tracing::info!("benchmark sampler shut down");
                    return;
                }
                Some(bench_req) = self.rx.recv() => {
                    self.process_request(bench_req, &sem).await;
                }
            }
        }
    }

    async fn process_request(&self, bench_req: BenchmarkRequest, sem: &Arc<Semaphore>) {
        let benchmark_models = self.select_benchmark_models(&bench_req.original_model);

        if benchmark_models.is_empty() {
            tracing::debug!("no benchmark models available, skipping");
            return;
        }

        let mut set = JoinSet::new();

        for (model_name, model_id, provider_name) in benchmark_models {
            let sem = sem.clone();
            let providers = self.providers.clone();
            let judge = self.judge.clone();
            let config = self.config.clone();
            let result_tx = self.result_tx.clone();
            let bench_req = bench_req.clone();
            let judge_model = self.config.benchmark.judge_model.clone();
            let fitness_cache = self.fitness_cache.clone();

            set.spawn(async move {
                let _permit = match sem.acquire().await {
                    Ok(p) => p,
                    Err(_) => return,
                };

                let event = run_single_benchmark(
                    &providers,
                    &judge,
                    &config,
                    &bench_req,
                    &model_name,
                    &model_id,
                    &provider_name,
                    &judge_model,
                )
                .await;

                if event.status == "success" {
                    if let Some(task_type) = event.task_type {
                        let bm_model = event.benchmark_model.clone();
                        let orig_model = event.original_model.clone();
                        let bm_score = event.benchmark_score;
                        let bm_cost = event.benchmark_cost;
                        let bm_latency = event.benchmark_latency_ms as f64;
                        let orig_score = event.original_score;
                        let cache = fitness_cache.clone();
                        tokio::spawn(async move {
                            cache
                                .record_benchmark(
                                    task_type, &bm_model, bm_score, bm_cost, bm_latency,
                                )
                                .await;
                            cache
                                .record_benchmark(task_type, &orig_model, orig_score, 0.0, 0.0)
                                .await;
                        });
                    }
                }

                let _ = result_tx.send(event).await;
            });
        }

        // Wait for all benchmarks to complete
        while set.join_next().await.is_some() {}
    }

    fn select_benchmark_models(&self, original_model: &str) -> Vec<(String, String, String)> {
        let available_providers: HashSet<&str> = self.providers.list().into_iter().collect();

        // Resolve original model's provider model_id for comparison
        let original_model_id = crate::proxy::handler::resolve_model(&self.config, original_model)
            .map(|(_, id)| id)
            .unwrap_or_default();

        // Collect candidates from MODEL_CATALOG, excluding original model
        let mut candidates: Vec<(&str, &crate::models::ModelInfo)> = MODEL_CATALOG
            .iter()
            .filter(|(_, info)| info.model_id != original_model_id)
            .filter(|(_, info)| available_providers.contains(info.provider))
            .map(|(name, info)| (*name, info))
            .collect();

        // Sort by tier for deterministic selection
        candidates.sort_by_key(|(_, info)| info.tier);

        let max = self.config.benchmark.max_benchmark_models;
        let mut selected = Vec::new();
        let mut seen_tiers: HashSet<u8> = HashSet::new();

        // First pass: one per tier
        for &(name, info) in &candidates {
            if selected.len() >= max {
                break;
            }
            if seen_tiers.insert(info.tier) {
                selected.push((name, info));
            }
        }

        // Second pass: fill remaining slots
        for &(name, info) in &candidates {
            if selected.len() >= max {
                break;
            }
            if !selected.iter().any(|(n, _)| *n == name) {
                selected.push((name, info));
            }
        }

        // Resolve each to (model_name, model_id, provider_name)
        selected
            .into_iter()
            .filter_map(|(name, _info)| {
                crate::proxy::handler::resolve_model(&self.config, name)
                    .ok()
                    .map(|(provider, model_id)| (name.to_string(), model_id, provider))
            })
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_single_benchmark(
    providers: &Arc<ProviderRegistry>,
    judge: &Judge,
    config: &Config,
    bench_req: &BenchmarkRequest,
    model_name: &str,
    model_id: &str,
    provider_name: &str,
    judge_model: &str,
) -> BenchmarkEvent {
    let start = Instant::now();
    let provider = match providers.get(provider_name) {
        Ok(p) => p,
        Err(_) => {
            return make_event(
                bench_req,
                model_name,
                judge_model,
                0.0,
                0.0,
                0.0,
                0,
                0.0,
                "benchmark_failed",
            );
        }
    };

    // Clone request, set stream: false
    let mut benchmark_request = bench_req.request.clone();
    benchmark_request.model = model_name.to_string();
    benchmark_request.stream = false;
    benchmark_request.stream_options = None;

    let response = match provider.chat_completion(&benchmark_request, model_id).await {
        Ok(crate::types::ProviderResponse::Complete(resp)) => resp,
        _ => {
            let latency = start.elapsed().as_millis() as u32;
            return make_event(
                bench_req,
                model_name,
                judge_model,
                0.0,
                0.0,
                0.0,
                latency,
                0.0,
                "benchmark_failed",
            );
        }
    };

    let benchmark_latency_ms = start.elapsed().as_millis() as u32;

    // Extract benchmark completion text
    let benchmark_completion: String = response
        .choices
        .iter()
        .filter_map(|c| c.message.content.as_ref().and_then(|v| v.as_str()))
        .collect();

    let usage = response.usage.unwrap_or_default();
    let benchmark_cost = compute_cost(model_name, &usage);

    // Call judge
    match judge
        .score(
            providers,
            config,
            bench_req.task_type,
            &bench_req.request.messages,
            &bench_req.original_completion,
            &benchmark_completion,
        )
        .await
    {
        Ok(judge_result) => make_event(
            bench_req,
            model_name,
            judge_model,
            judge_result.original_score,
            judge_result.benchmark_score,
            benchmark_cost,
            benchmark_latency_ms,
            judge_result.judge_cost,
            "success",
        ),
        Err(e) => {
            tracing::warn!(
                error = %e,
                benchmark_model = model_name,
                "judge scoring failed"
            );
            make_event(
                bench_req,
                model_name,
                judge_model,
                0.0,
                0.0,
                benchmark_cost,
                benchmark_latency_ms,
                0.0,
                "judge_failed",
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn make_event(
    bench_req: &BenchmarkRequest,
    benchmark_model: &str,
    judge_model: &str,
    original_score: f64,
    benchmark_score: f64,
    benchmark_cost: f64,
    benchmark_latency_ms: u32,
    judge_cost: f64,
    status: &str,
) -> BenchmarkEvent {
    BenchmarkEvent {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        inference_id: bench_req.inference_id,
        task_type: bench_req.task_type,
        original_model: bench_req.original_model.clone(),
        benchmark_model: benchmark_model.to_string(),
        judge_model: judge_model.to_string(),
        original_score,
        benchmark_score,
        benchmark_cost,
        benchmark_latency_ms,
        judge_cost,
        prompt_hash: bench_req.prompt_hash.clone(),
        status: status.to_string(),
    }
}
