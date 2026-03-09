use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{PrismError, Result};
use crate::keys::MasterAuth;
use crate::proxy::handler::AppState;

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub key_id: Option<Uuid>,
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    100
}

#[derive(Debug, Serialize)]
pub struct AuditEvent {
    pub id: Uuid,
    pub event_type: String,
    pub key_id: Option<Uuid>,
    pub key_prefix: Option<String>,
    pub actor: Option<String>,
    pub details: serde_json::Value,
    pub ip_addr: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// GET /api/v1/audit — list audit events (MasterAuth required).
pub async fn list_audit_events(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEvent>>> {
    let key_service = state
        .key_service
        .as_ref()
        .ok_or_else(|| PrismError::Internal("keys not enabled".into()))?;

    let pool = key_service.repo().pool();

    let rows = sqlx::query_as::<_, AuditEventRow>(
        r#"SELECT id, event_type, key_id, key_prefix, actor, details, ip_addr, created_at
           FROM audit_events
           WHERE ($1::uuid IS NULL OR key_id = $1)
             AND ($2::text IS NULL OR event_type = $2)
             AND ($3::timestamptz IS NULL OR created_at >= $3)
             AND ($4::timestamptz IS NULL OR created_at <= $4)
           ORDER BY created_at DESC
           LIMIT $5 OFFSET $6"#,
    )
    .bind(query.key_id)
    .bind(&query.event_type)
    .bind(query.since)
    .bind(query.until)
    .bind(query.limit)
    .bind(query.offset)
    .fetch_all(pool)
    .await
    .map_err(|e| PrismError::Internal(format!("audit query failed: {e}")))?;

    Ok(Json(rows.into_iter().map(AuditEvent::from).collect()))
}

#[derive(sqlx::FromRow)]
struct AuditEventRow {
    id: Uuid,
    event_type: String,
    key_id: Option<Uuid>,
    key_prefix: Option<String>,
    actor: Option<String>,
    details: serde_json::Value,
    ip_addr: Option<String>,
    created_at: DateTime<Utc>,
}

impl From<AuditEventRow> for AuditEvent {
    fn from(r: AuditEventRow) -> Self {
        Self {
            id: r.id,
            event_type: r.event_type,
            key_id: r.key_id,
            key_prefix: r.key_prefix,
            actor: r.actor,
            details: r.details,
            ip_addr: r.ip_addr,
            created_at: r.created_at,
        }
    }
}
