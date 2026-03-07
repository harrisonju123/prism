use std::sync::Arc;

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::proxy::handler::AppState;

pub async fn metrics(State(state): State<Arc<AppState>>) -> Response {
    let body = if let Some(ref collector) = state.metrics {
        collector.render_prometheus()
    } else {
        "# No metrics collector configured\n".to_string()
    };

    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}
