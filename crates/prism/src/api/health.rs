use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::proxy::handler::AppState;

static START_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn init_start_time() {
    START_TIME.get_or_init(std::time::Instant::now);
}

#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
    version: &'static str,
    uptime_secs: u64,
    circuit_breakers: HashMap<String, String>,
}

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Gateway is healthy"),
    ),
    tag = "health"
)]
pub async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let uptime = START_TIME.get().map(|t| t.elapsed().as_secs()).unwrap_or(0);

    // Collect circuit breaker states
    let mut circuit_breakers = HashMap::new();
    for entry in state.circuit_breakers.iter() {
        let name = entry.key().clone();
        let cb_state = entry.value().state_name().to_string();
        circuit_breakers.insert(name, cb_state);
    }

    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: uptime,
        circuit_breakers,
    })
}

#[utoipa::path(
    get,
    path = "/health/live",
    responses(
        (status = 200, description = "Service is alive"),
    ),
    tag = "health"
)]
pub async fn liveness() -> &'static str {
    "ok"
}

#[derive(Serialize)]
pub struct ReadinessResponse {
    status: String,
    checks: ReadinessChecks,
}

#[derive(Serialize)]
pub struct ReadinessChecks {
    postgres: CheckResult,
    clickhouse: CheckResult,
    redis: CheckResult,
}

#[derive(Serialize)]
pub struct CheckResult {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl CheckResult {
    fn ok() -> Self {
        Self {
            status: "ok".into(),
            message: None,
        }
    }

    fn unavailable(msg: String) -> Self {
        Self {
            status: "unavailable".into(),
            message: Some(msg),
        }
    }

    fn not_configured() -> Self {
        Self {
            status: "not_configured".into(),
            message: None,
        }
    }
}

#[utoipa::path(
    get,
    path = "/health/ready",
    responses(
        (status = 200, description = "Service is ready"),
        (status = 503, description = "Service is unavailable"),
    ),
    tag = "health"
)]
pub async fn readiness(State(state): State<Arc<AppState>>) -> Response {
    let timeout = Duration::from_secs(2);

    // Check Postgres
    #[cfg(feature = "postgres")]
    let pg_check = if let Some(ref ks) = state.key_service {
        let pool = ks.repo().pool();
        match tokio::time::timeout(timeout, sqlx::query("SELECT 1").execute(pool)).await {
            Ok(Ok(_)) => CheckResult::ok(),
            Ok(Err(e)) => CheckResult::unavailable(format!("{e}")),
            Err(_) => CheckResult::unavailable("timeout".into()),
        }
    } else {
        CheckResult::not_configured()
    };
    #[cfg(not(feature = "postgres"))]
    let pg_check = CheckResult::not_configured();

    // Check ClickHouse
    let ch_check = {
        let client = reqwest::Client::new();
        let url = &state.config.clickhouse.url;
        match tokio::time::timeout(timeout, client.get(format!("{url}/?query=SELECT+1")).send())
            .await
        {
            Ok(Ok(resp)) if resp.status().is_success() => CheckResult::ok(),
            Ok(Ok(resp)) => CheckResult::unavailable(format!("HTTP {}", resp.status())),
            Ok(Err(e)) => CheckResult::unavailable(format!("{e}")),
            Err(_) => CheckResult::unavailable("timeout".into()),
        }
    };

    // Check Redis
    #[cfg(feature = "redis-backend")]
    let redis_check =
        if state.config.rate_limit.backend == "redis" || state.config.cache.backend == "redis" {
            let url = state
                .config
                .rate_limit
                .redis_url
                .as_deref()
                .unwrap_or("redis://127.0.0.1:6379");
            match tokio::time::timeout(timeout, check_redis(url)).await {
                Ok(Ok(())) => CheckResult::ok(),
                Ok(Err(e)) => CheckResult::unavailable(format!("{e}")),
                Err(_) => CheckResult::unavailable("timeout".into()),
            }
        } else {
            CheckResult::not_configured()
        };
    #[cfg(not(feature = "redis-backend"))]
    let redis_check = CheckResult::not_configured();

    // Determine overall status
    let pg_critical = state.key_service.is_some() && pg_check.status != "ok";
    let ch_down = ch_check.status == "unavailable";
    let redis_down = redis_check.status == "unavailable";

    let (status, http_status) = if pg_critical {
        ("unavailable", StatusCode::SERVICE_UNAVAILABLE)
    } else if ch_down || redis_down {
        ("degraded", StatusCode::OK)
    } else {
        ("ok", StatusCode::OK)
    };

    let body = ReadinessResponse {
        status: status.into(),
        checks: ReadinessChecks {
            postgres: pg_check,
            clickhouse: ch_check,
            redis: redis_check,
        },
    };

    (http_status, Json(body)).into_response()
}

/// GET /health/providers — public, no auth required.
pub async fn provider_health(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<crate::providers::health::ProviderHealth>> {
    let snapshot = state
        .health_tracker
        .as_ref()
        .map(|ht| ht.snapshot())
        .unwrap_or_default();
    Json(snapshot)
}

#[cfg(feature = "redis-backend")]
async fn check_redis(url: &str) -> std::result::Result<(), String> {
    let client = redis::Client::open(url).map_err(|e| e.to_string())?;
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| e.to_string())?;
    redis::cmd("PING")
        .query_async::<String>(&mut conn)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn liveness_returns_ok() {
        let result = liveness().await;
        assert_eq!(result, "ok");
    }

    #[tokio::test]
    async fn health_returns_version() {
        use crate::providers::ProviderRegistry;
        use crate::proxy::builder::AppStateBuilder;
        use std::collections::HashMap;

        init_start_time();
        let providers = Arc::new(ProviderRegistry::from_config(
            &HashMap::new(),
            reqwest::Client::new(),
        ));
        let (event_tx, _rx) = tokio::sync::mpsc::channel(1);
        let state = AppStateBuilder::new(figment::Figment::new().extract().unwrap())
            .with_providers(providers)
            .with_event_tx(event_tx)
            .build()
            .unwrap();
        let state = Arc::new(state);
        let Json(resp) = health(State(state)).await;
        assert_eq!(resp.status, "ok");
        assert_eq!(resp.version, env!("CARGO_PKG_VERSION"));
        assert!(resp.uptime_secs < 5); // just started
        assert!(resp.circuit_breakers.is_empty()); // no providers active
    }

    #[test]
    fn check_result_ok() {
        let r = CheckResult::ok();
        assert_eq!(r.status, "ok");
        assert!(r.message.is_none());
    }

    #[test]
    fn check_result_unavailable() {
        let r = CheckResult::unavailable("connection refused".into());
        assert_eq!(r.status, "unavailable");
        assert!(r.message.as_deref().unwrap().contains("connection refused"));
    }
}
