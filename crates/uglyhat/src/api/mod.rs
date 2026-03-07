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
