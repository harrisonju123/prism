use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, ProviderConfig};
use crate::error::{PrismError, Result};
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
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<InferenceEvent>(512);

    let state = Arc::new(
        configure(
            AppStateBuilder::new(config)
                .with_providers(registry)
                .with_event_tx(event_tx),
        )
        .build()
        .map_err(|e| PrismError::Internal(e.to_string()))?,
    );

    let session_cost = state.session_cost_usd.clone();
    let router = crate::server::router::build(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| PrismError::Internal(e.to_string()))?;
    let addr = listener
        .local_addr()
        .map_err(|e| PrismError::Internal(e.to_string()))?;

    let cancel = CancellationToken::new();
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
        session_cost,
    })
}
