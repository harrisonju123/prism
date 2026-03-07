use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::Result;
use crate::model::*;
use crate::store::ActivityFilters;

impl SqliteStore {
    pub(crate) async fn log_activity_impl(
        &self,
        workspace_id: Uuid,
        actor: &str,
        action: &str,
        entity_type: &str,
        entity_id: Uuid,
        summary: &str,
        detail: Option<serde_json::Value>,
    ) -> Result<()> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO activity_log (id, workspace_id, actor, action, entity_type, entity_id, summary, detail, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(actor)
        .bind(action)
        .bind(entity_type)
        .bind(entity_id.to_string())
        .bind(summary)
        .bind(opt_value_to_str(&detail))
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn create_activity_impl(
        &self,
        workspace_id: Uuid,
        actor: &str,
        action: &str,
        entity_type: &str,
        entity_id: Uuid,
        summary: &str,
        detail: Option<serde_json::Value>,
    ) -> Result<ActivityEntry> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO activity_log (id, workspace_id, actor, action, entity_type, entity_id, summary, detail, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             RETURNING id, workspace_id, actor, action, entity_type, entity_id, summary, detail, created_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(actor)
        .bind(action)
        .bind(entity_type)
        .bind(entity_id.to_string())
        .bind(summary)
        .bind(opt_value_to_str(&detail))
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        row_to_activity_entry(&row)
    }

    pub(crate) async fn list_activity_impl(
        &self,
        workspace_id: Uuid,
        filters: ActivityFilters,
    ) -> Result<Vec<ActivityEntry>> {
        let mut clauses = vec!["workspace_id = $1".to_string()];
        let mut args: Vec<String> = vec![workspace_id.to_string()];

        if let Some(ref since) = filters.since {
            args.push(since.to_rfc3339());
            clauses.push(format!("created_at >= ${}", args.len()));
        }
        if let Some(ref actor) = filters.actor {
            args.push(actor.clone());
            clauses.push(format!("actor = ${}", args.len()));
        }
        if let Some(ref entity_type) = filters.entity_type {
            args.push(entity_type.clone());
            clauses.push(format!("entity_type = ${}", args.len()));
        }

        let limit = if filters.limit > 0 && filters.limit <= 200 {
            filters.limit
        } else {
            50
        };

        let query = format!(
            "SELECT id, workspace_id, actor, action, entity_type, entity_id, summary, detail, created_at
             FROM activity_log
             WHERE {}
             ORDER BY created_at DESC
             LIMIT {}",
            clauses.join(" AND "),
            limit,
        );

        let mut q = sqlx::query(&query);
        for arg in &args {
            q = q.bind(arg);
        }

        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_activity_entry).collect()
    }

    pub(crate) async fn list_activity_since_impl(
        &self,
        workspace_id: Uuid,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ActivityEntry>> {
        let actual_limit = if limit <= 0 { 50 } else { limit };
        let rows = sqlx::query(
            "SELECT id, workspace_id, actor, action, entity_type, entity_id, summary, detail, created_at
             FROM activity_log
             WHERE workspace_id = $1 AND created_at >= $2
             ORDER BY created_at DESC
             LIMIT $3",
        )
        .bind(workspace_id.to_string())
        .bind(since.to_rfc3339())
        .bind(actual_limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_activity_entry).collect()
    }
}

pub(super) fn row_to_activity_entry(row: &sqlx::sqlite::SqliteRow) -> Result<ActivityEntry> {
    let id_str: String = row.try_get("id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let entity_id_str: String = row.try_get("entity_id")?;
    let det_str: Option<String> = row.try_get("detail")?;
    let created_str: String = row.try_get("created_at")?;

    Ok(ActivityEntry {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        actor: row.try_get("actor")?,
        action: row.try_get("action")?,
        entity_type: row.try_get("entity_type")?,
        entity_id: parse_uuid(&entity_id_str)?,
        summary: row.try_get("summary")?,
        detail: str_to_opt_value(det_str),
        created_at: parse_time(&created_str)?,
    })
}
