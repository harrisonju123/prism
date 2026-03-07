use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("internal: {0}")]
    Internal(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            Error::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            Error::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            Error::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            Error::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            Error::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m.clone()),
            Error::Sqlx(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        (status, msg).into_response()
    }
}
