use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

pub(super) const DECISION_SELECT: &str =
    "d.id, d.workspace_id, COALESCE(w.name, '') AS workspace_name,
     d.initiative_id, COALESCE(i.name, '') AS initiative_name,
     d.epic_id, COALESCE(ep.name, '') AS epic_name,
     d.title, d.content, d.status, d.metadata, d.created_at, d.updated_at";

pub(super) const DECISION_JOINS: &str = "FROM decisions d
     LEFT JOIN workspaces w ON w.id = d.workspace_id
     LEFT JOIN initiatives i ON i.id = d.initiative_id
     LEFT JOIN epics ep ON ep.id = d.epic_id";

impl SqliteStore {
    pub(crate) async fn create_decision_impl(
        &self,
        workspace_id: Option<Uuid>,
        initiative_id: Option<Uuid>,
        epic_id: Option<Uuid>,
        title: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Decision> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();

        sqlx::query(
            "INSERT INTO decisions (id, workspace_id, initiative_id, epic_id, title, content, status, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'proposed', $7, $8, $9)",
        )
        .bind(id.to_string())
        .bind(opt_uuid_to_str(workspace_id))
        .bind(opt_uuid_to_str(initiative_id))
        .bind(opt_uuid_to_str(epic_id))
        .bind(title)
        .bind(content)
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        let row = sqlx::query(&format!(
            "SELECT {DECISION_SELECT} {DECISION_JOINS} WHERE d.id = $1"
        ))
        .bind(id.to_string())
        .fetch_one(&self.pool)
        .await?;

        let d = row_to_decision(&row)?;
        let log_ws = d.workspace_id.unwrap_or(Uuid::nil());
        let _ = self
            .log_activity_impl(
                log_ws,
                "",
                "created",
                "decision",
                d.id,
                &format!("Created decision: {}", d.title),
                None,
            )
            .await;
        Ok(d)
    }

    pub(crate) async fn get_decision_impl(&self, id: Uuid) -> Result<Decision> {
        let row = sqlx::query(&format!(
            "SELECT {DECISION_SELECT} {DECISION_JOINS} WHERE d.id = $1"
        ))
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("decision {id} not found")))?;
        row_to_decision(&row)
    }

    pub(crate) async fn list_decisions_by_workspace_impl(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<Decision>> {
        let rows = sqlx::query(&format!(
            "SELECT {DECISION_SELECT} {DECISION_JOINS} WHERE d.workspace_id = $1 ORDER BY d.created_at DESC"
        ))
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_decision).collect()
    }

    pub(crate) async fn update_decision_impl(
        &self,
        id: Uuid,
        title: &str,
        content: &str,
        status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Decision> {
        let now = now_rfc3339();
        let result = sqlx::query(
            "UPDATE decisions SET title = $1, content = $2, status = $3, metadata = $4, updated_at = $5 WHERE id = $6",
        )
        .bind(title)
        .bind(content)
        .bind(status)
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("decision {id} not found")));
        }

        let row = sqlx::query(&format!(
            "SELECT {DECISION_SELECT} {DECISION_JOINS} WHERE d.id = $1"
        ))
        .bind(id.to_string())
        .fetch_one(&self.pool)
        .await?;

        let d = row_to_decision(&row)?;
        let log_ws = d.workspace_id.unwrap_or(Uuid::nil());
        let _ = self
            .log_activity_impl(
                log_ws,
                "",
                "updated",
                "decision",
                d.id,
                &format!("Updated decision: {}", d.title),
                None,
            )
            .await;
        Ok(d)
    }

    pub(crate) async fn delete_decision_impl(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM decisions WHERE id = $1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("decision {id} not found")));
        }
        Ok(())
    }
}

pub(super) fn row_to_decision(row: &sqlx::sqlite::SqliteRow) -> Result<Decision> {
    let id_str: String = row.try_get("id")?;
    let ws_id_str: Option<String> = row.try_get("workspace_id")?;
    let init_id_str: Option<String> = row.try_get("initiative_id")?;
    let epic_id_str: Option<String> = row.try_get("epic_id")?;
    let meta_str: Option<String> = row.try_get("metadata")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Decision {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_opt_uuid(ws_id_str)?,
        workspace_name: row.try_get("workspace_name").unwrap_or_default(),
        initiative_id: parse_opt_uuid(init_id_str)?,
        initiative_name: row.try_get("initiative_name").unwrap_or_default(),
        epic_id: parse_opt_uuid(epic_id_str)?,
        epic_name: row.try_get("epic_name").unwrap_or_default(),
        title: row.try_get("title")?,
        content: row.try_get("content")?,
        status: row.try_get("status")?,
        metadata: str_to_opt_value(meta_str),
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}
