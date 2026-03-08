pub mod activity;
pub mod agent;
pub mod apikey;
pub mod context;
pub mod decision;
pub mod dependency;
pub mod epic;
pub mod handoff;
pub mod health;
pub mod initiative;
pub mod note;
pub mod task;
pub mod workspace;

use std::sync::Arc;

use crate::store::Store;

pub struct AppState {
    pub store: Arc<dyn Store + Send + Sync>,
}

pub(crate) fn parse_rfc3339_param(
    s: &str,
) -> Result<chrono::DateTime<chrono::Utc>, crate::error::Error> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| crate::error::Error::BadRequest(format!("invalid timestamp: {e}")))
}
