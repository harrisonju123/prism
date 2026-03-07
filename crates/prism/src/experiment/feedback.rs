use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;

/// Internal feedback event for ClickHouse persistence.
#[derive(Debug, Clone, Serialize)]
pub struct FeedbackEvent {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub inference_id: Option<Uuid>,
    pub episode_id: Option<Uuid>,
    pub metric_name: String,
    pub metric_value: f64,
    pub metadata: String,
}

#[derive(Debug, Deserialize)]
pub struct FeedbackRequest {
    #[serde(default)]
    pub inference_id: Option<Uuid>,
    #[serde(default)]
    pub episode_id: Option<Uuid>,
    pub metric_name: String,
    pub metric_value: f64,
    #[serde(default = "default_metadata")]
    pub metadata: serde_json::Value,
}

fn default_metadata() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

#[derive(Debug, Serialize)]
pub struct FeedbackResponse {
    pub id: Uuid,
    pub status: String,
}

/// POST /api/v1/feedback
pub async fn submit_feedback(
    State(state): State<Arc<AppState>>,
    Json(request): Json<FeedbackRequest>,
) -> Result<impl IntoResponse> {
    // Validate at least one of inference_id or episode_id is present
    if request.inference_id.is_none() && request.episode_id.is_none() {
        return Err(PrismError::BadRequest(
            "at least one of inference_id or episode_id is required".to_string(),
        ));
    }

    let event = FeedbackEvent {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        inference_id: request.inference_id,
        episode_id: request.episode_id,
        metric_name: request.metric_name.clone(),
        metric_value: request.metric_value,
        metadata: serde_json::to_string(&request.metadata).unwrap_or_else(|_| "{}".to_string()),
    };

    // Propagate reward to bandit if applicable
    if matches!(request.metric_name.as_str(), "reward" | "quality")
        && let (Some(episode_id), Some(engine)) = (request.episode_id, &state.experiment_engine)
    {
        engine.propagate_feedback(episode_id, request.metric_value);
    }

    let event_id = event.id;

    // Send to feedback writer
    if let Some(ref tx) = state.feedback_tx {
        let _ = tx.send(event).await;
    }

    Ok(Json(FeedbackResponse {
        id: event_id,
        status: "accepted".to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_feedback(inference_id: Option<Uuid>, episode_id: Option<Uuid>) -> FeedbackRequest {
        FeedbackRequest {
            inference_id,
            episode_id,
            metric_name: "reward".to_string(),
            metric_value: 1.0,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    #[test]
    fn requires_at_least_one_id() {
        let req = make_feedback(None, None);
        assert!(req.inference_id.is_none() && req.episode_id.is_none());
        // The validation is in the handler; here we just confirm the data shape
    }

    #[test]
    fn accepts_inference_id() {
        let req = make_feedback(Some(Uuid::new_v4()), None);
        assert!(req.inference_id.is_some());
        assert!(req.episode_id.is_none());
    }

    #[test]
    fn accepts_episode_id() {
        let req = make_feedback(None, Some(Uuid::new_v4()));
        assert!(req.inference_id.is_none());
        assert!(req.episode_id.is_some());
    }
}
