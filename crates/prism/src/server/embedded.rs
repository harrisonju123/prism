use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, ProviderConfig};
use crate::error::{PrismError, Result};
use crate::observability::local_writer::{LocalInferenceWriter, spawn_event_consumer};
use crate::observability::metrics::MetricsCollector;
use crate::providers::ProviderRegistry;
use crate::proxy::builder::AppStateBuilder;
use crate::types::InferenceEvent;

/// A PrisM gateway running in-process on a random loopback port.
///
/// Drop to shut down the server gracefully.
pub struct EmbeddedGateway {
    addr: SocketAddr,
    cancel: CancellationToken,
    _task: tokio::task::JoinHandle<()>,
    _event_task: tokio::task::JoinHandle<()>,
    _pruner_task: Option<tokio::task::JoinHandle<()>>,
    session_cost: Arc<std::sync::atomic::AtomicU64>,
}

impl EmbeddedGateway {
    /// The local address the gateway is bound to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Base URL for the OpenAI-compatible API, e.g. `http://127.0.0.1:PORT/v1`.
    pub fn api_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.addr.port())
    }

    /// Access to the embedded gateway's session cost counter.
    pub fn session_cost_usd(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.session_cost.clone()
    }
}

impl Drop for EmbeddedGateway {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Start a minimal PrisM gateway in the current process on a random loopback port.
///
/// Provider keys are read from `PRISM_*` environment variables on startup
/// (via [`Config::load`]). Additional providers can be passed as
/// `(name, api_key, api_base)` tuples to override or extend the env config.
///
/// # Example
/// ```no_run
/// # async fn example() -> anyhow::Result<()> {
/// let gw = prism::start_embedded(std::iter::empty()).await?;
/// println!("Listening on {}", gw.api_url());
/// # Ok(())
/// # }
/// ```
pub async fn start_embedded(
    providers: impl IntoIterator<Item = (String, String, String)>,
) -> Result<EmbeddedGateway> {
    start_embedded_with(providers, |b| b).await
}

/// Start a minimal PrisM gateway with custom AppStateBuilder configuration.
///
/// Provider keys are read from `PRISM_*` environment variables on startup
/// (via [`Config::load`]). Additional providers can be passed as
/// `(name, api_key, api_base)` tuples to override or extend the env config.
///
/// The `configure` closure allows customization of the `AppStateBuilder` before
/// building the final `AppState`. This enables advanced use cases like routing
/// policy overrides or custom metrics injection.
///
/// # Example
/// ```no_run
/// # async fn example() -> anyhow::Result<()> {
/// let gw = prism::start_embedded_with(
///     vec![("anthropic".into(), "sk-ant-...".into(), "https://api.anthropic.com/v1".into())],
///     |builder| builder, // customize here if needed
/// ).await?;
/// println!("Listening on {}", gw.api_url());
/// # Ok(())
/// # }
/// ```
pub async fn start_embedded_with(
    providers: impl IntoIterator<Item = (String, String, String)>,
    configure: impl FnOnce(AppStateBuilder) -> AppStateBuilder,
) -> Result<EmbeddedGateway> {
    // Load config from env / TOML; fall back to pure defaults if load fails.
    let mut config = Config::load(None).unwrap_or_else(|_| Config::default());

    // Caller-supplied providers override env config.
    for (name, api_key, api_base) in providers {
        config.providers.insert(
            name,
            ProviderConfig {
                api_key: Some(api_key),
                api_base: Some(api_base),
                provider_type: None,
                region: None,
                extra: HashMap::new(),
                prompt_caching: true,
            },
        );
    }

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.streaming.request_timeout_secs))
        .build()
        .map_err(|e| PrismError::Internal(e.to_string()))?;

    let registry = Arc::new(ProviderRegistry::from_config(
        &config.providers,
        http_client,
    ));
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<InferenceEvent>(512);

    // --- Observability: MetricsCollector + LocalInferenceWriter ---
    let metrics = Arc::new(MetricsCollector::new());

    let local_writer = open_local_writer().await;

    let cancel = CancellationToken::new();

    let event_task = spawn_event_consumer(
        event_rx,
        metrics.clone(),
        local_writer.clone(),
        cancel.clone(),
    );

    let state = Arc::new(
        configure(
            AppStateBuilder::new(config)
                .with_providers(registry)
                .with_event_tx(event_tx)
                .with_metrics(metrics)
                .with_local_inference_writer(local_writer),
        )
        .build()
        .map_err(|e| PrismError::Internal(e.to_string()))?,
    );

    let session_cost = state.session_cost_usd.clone();

    // Spawn hourly retention pruner (30-day default) only when writer is available.
    let pruner_task = state.local_inference_writer.clone().map(|pruner_writer| {
        let pruner_cancel = cancel.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = pruner_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
                        match pruner_writer.prune_before(cutoff).await {
                            Ok(n) if n > 0 => tracing::info!(rows = n, "pruned old inference events"),
                            Err(e) => tracing::warn!(error = %e, "inference event pruning failed"),
                            _ => {}
                        }
                    }
                }
            }
        })
    });

    let router = crate::server::router::build(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| PrismError::Internal(e.to_string()))?;
    let addr = listener
        .local_addr()
        .map_err(|e| PrismError::Internal(e.to_string()))?;

    let shutdown_token = cancel.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_token.cancelled_owned())
            .await
            .ok();
    });

    tracing::info!(%addr, "PrisM embedded gateway started");
    Ok(EmbeddedGateway {
        addr,
        cancel,
        _task: task,
        _event_task: event_task,
        _pruner_task: pruner_task,
        session_cost,
    })
}

/// Try to open the local inference DB, returning None on failure.
async fn open_local_writer() -> Option<Arc<LocalInferenceWriter>> {
    let path = find_local_db_path();
    match LocalInferenceWriter::open(&path).await {
        Ok(w) => {
            tracing::info!(path = %path.display(), "local inference store opened");
            Some(w)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "could not open local inference store — waste detection and local stats unavailable"
            );
            None
        }
    }
}

/// Resolve the path for the local inference DB.
///
/// Walk up from CWD looking for `.prism/` directory; fall back to
/// `~/.prism/observability.db`.
fn find_local_db_path() -> std::path::PathBuf {
    // Walk up from CWD
    let mut dir = std::env::current_dir().unwrap_or_default();
    loop {
        let candidate = dir.join(".prism");
        if candidate.is_dir() {
            return candidate.join("observability.db");
        }
        if !dir.pop() {
            break;
        }
    }
    // Fallback: home dir
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".prism")
        .join("observability.db")
}
