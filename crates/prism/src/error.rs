use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum PrismError {
    #[error("provider error: {0}")]
    Provider(String),

    #[error("upstream timeout after {0}ms")]
    Timeout(u64),

    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("provider not configured: {0}")]
    ProviderNotConfigured(String),

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("rate limit exceeded")]
    RateLimited { retry_after_secs: Option<u64> },

    #[error("budget exceeded")]
    BudgetExceeded,

    #[error("authentication required")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("schema validation failed: {0}")]
    SchemaValidationFailed(String),

    #[error("content filtered: {0}")]
    ContentFiltered(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),

    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
}

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    message: String,
    r#type: &'static str,
    code: Option<String>,
}

impl IntoResponse for PrismError {
    fn into_response(self) -> Response {
        let (status, error_type, message) = match &self {
            PrismError::Provider(msg) => (StatusCode::BAD_GATEWAY, "provider_error", msg.clone()),
            PrismError::Timeout(ms) => (
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                format!("upstream timeout after {ms}ms"),
            ),
            PrismError::ModelNotFound(m) => (
                StatusCode::NOT_FOUND,
                "model_not_found",
                format!("model not found: {m}"),
            ),
            PrismError::ProviderNotConfigured(p) => (
                StatusCode::BAD_REQUEST,
                "provider_not_configured",
                format!("provider not configured: {p}"),
            ),
            PrismError::BadRequest(msg) => {
                (StatusCode::BAD_REQUEST, "invalid_request", msg.clone())
            }
            PrismError::RateLimited { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_exceeded",
                "rate limit exceeded".into(),
            ),
            PrismError::BudgetExceeded => (
                StatusCode::PAYMENT_REQUIRED,
                "budget_exceeded",
                "budget exceeded".into(),
            ),
            PrismError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "authentication required".into(),
            ),
            PrismError::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "access denied".into()),
            PrismError::SchemaValidationFailed(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "schema_validation_failed",
                msg.clone(),
            ),
            PrismError::ContentFiltered(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "content_filtered",
                msg.clone(),
            ),
            PrismError::Internal(msg) => {
                tracing::error!(error = %msg, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal server error".into(),
                )
            }
            PrismError::Reqwest(e) => {
                tracing::error!(error = %e, "reqwest error");
                (
                    StatusCode::BAD_GATEWAY,
                    "provider_error",
                    format!("upstream error: {e}"),
                )
            }
            PrismError::SerdeJson(e) => (
                StatusCode::BAD_REQUEST,
                "parse_error",
                format!("JSON parse error: {e}"),
            ),
        };

        let body = ErrorResponse {
            error: ErrorBody {
                message,
                r#type: error_type,
                code: None,
            },
        };

        let mut response = (status, axum::Json(body)).into_response();

        // Add Retry-After header for rate-limited responses
        if let PrismError::RateLimited {
            retry_after_secs: Some(secs),
        } = &self
        {
            response.headers_mut().insert(
                "retry-after",
                axum::http::HeaderValue::from_str(&secs.to_string()).unwrap(),
            );
        }

        response
    }
}

impl PrismError {
    pub fn is_retryable(&self) -> bool {
        match self {
            PrismError::Timeout(_) => true,
            PrismError::Provider(msg) => {
                msg.contains("500")
                    || msg.contains("502")
                    || msg.contains("503")
                    || msg.contains("504")
                    || msg.contains("529")
            }
            PrismError::Reqwest(e) => e.is_connect() || e.is_timeout(),
            PrismError::Internal(_) => false,
            PrismError::ModelNotFound(_)
            | PrismError::ProviderNotConfigured(_)
            | PrismError::BadRequest(_)
            | PrismError::RateLimited { .. }
            | PrismError::BudgetExceeded
            | PrismError::Unauthorized
            | PrismError::Forbidden
            | PrismError::SchemaValidationFailed(_)
            | PrismError::ContentFiltered(_)
            | PrismError::SerdeJson(_) => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, PrismError>;
