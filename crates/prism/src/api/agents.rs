use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;
use prism_context::model::{Handoff, HandoffStatus, InboxEntry, InboxEntryType, WorkPackage};
use prism_context::store::{InboxFilters, Store};

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct InboxParams {
    pub unread: Option<bool>,
    #[serde(rename = "type")]
    pub entry_type: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct HandoffParams {
    pub agent: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkPackageParams {
    pub plan_id: Option<String>,
    pub status: Option<String>,
}

// ---------------------------------------------------------------------------
// Response wrappers
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct InboxListResponse {
    pub entries: Vec<InboxEntry>,
    pub total: usize,
}

#[derive(Serialize)]
pub struct HandoffListResponse {
    pub handoffs: Vec<Handoff>,
    pub total: usize,
}

#[derive(Serialize)]
pub struct WorkPackageListResponse {
    pub packages: Vec<WorkPackage>,
    pub total: usize,
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn uh_store(state: &AppState) -> Result<(&dyn Store, Uuid)> {
    let (store, ws_id) = state
        .uh_store
        .as_ref()
        .zip(state.uh_workspace_id)
        .ok_or_else(|| {
            PrismError::Internal("uglyhat store not available — is .uglyhat.db present?".into())
        })?;
    Ok((store.as_ref(), ws_id))
}

// ---------------------------------------------------------------------------
// Inbox handlers
// ---------------------------------------------------------------------------

pub async fn list_inbox(
    State(state): State<Arc<AppState>>,
    Query(params): Query<InboxParams>,
) -> Result<impl IntoResponse> {
    let (store, ws_id) = uh_store(&state)?;

    let entry_type = params
        .entry_type
        .as_deref()
        .and_then(InboxEntryType::from_str);

    let filters = InboxFilters {
        unread_only: params.unread.unwrap_or(false),
        entry_type,
        include_dismissed: false,
        limit: params.limit.unwrap_or(100).clamp(1, 500),
    };

    let entries = store
        .list_inbox_entries(ws_id, filters)
        .await
        .map_err(|e| PrismError::Internal(e.to_string()))?;

    let total = entries.len();
    Ok(Json(InboxListResponse { entries, total }))
}

pub async fn mark_inbox_read(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let (store, ws_id) = uh_store(&state)?;
    store
        .mark_inbox_read(ws_id, id)
        .await
        .map_err(|e| PrismError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

pub async fn dismiss_inbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let (store, ws_id) = uh_store(&state)?;
    store
        .dismiss_inbox_entry(ws_id, id)
        .await
        .map_err(|e| PrismError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

// ---------------------------------------------------------------------------
// Handoff handlers
// ---------------------------------------------------------------------------

pub async fn list_handoffs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HandoffParams>,
) -> Result<impl IntoResponse> {
    let (store, ws_id) = uh_store(&state)?;

    let status = params.status.as_deref().and_then(HandoffStatus::from_str);

    let handoffs = store
        .list_handoffs(ws_id, params.agent.as_deref(), status)
        .await
        .map_err(|e| PrismError::Internal(e.to_string()))?;

    let total = handoffs.len();
    Ok(Json(HandoffListResponse { handoffs, total }))
}

// ---------------------------------------------------------------------------
// Work package handlers
// ---------------------------------------------------------------------------

pub async fn list_work_packages(
    State(state): State<Arc<AppState>>,
    Query(params): Query<WorkPackageParams>,
) -> Result<impl IntoResponse> {
    use prism_context::model::WorkPackageStatus;

    let (store, ws_id) = uh_store(&state)?;

    let plan_id = params
        .plan_id
        .as_deref()
        .and_then(|s| s.parse::<Uuid>().ok());

    let status = params.status.as_deref().and_then(|s| {
        Some(match s {
            "draft" => WorkPackageStatus::Draft,
            "planned" => WorkPackageStatus::Planned,
            "ready" => WorkPackageStatus::Ready,
            "in_progress" => WorkPackageStatus::InProgress,
            "review" => WorkPackageStatus::Review,
            "done" => WorkPackageStatus::Done,
            "cancelled" => WorkPackageStatus::Cancelled,
            _ => return None,
        })
    });

    let packages = store
        .list_work_packages(ws_id, plan_id, status)
        .await
        .map_err(|e| PrismError::Internal(e.to_string()))?;

    let total = packages.len();
    Ok(Json(WorkPackageListResponse { packages, total }))
}
