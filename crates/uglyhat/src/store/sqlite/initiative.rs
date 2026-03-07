use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn create_initiative_impl(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Initiative> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO initiatives (id, workspace_id, name, description, status, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'active', $5, $6, $7)
             RETURNING id, workspace_id,
               (SELECT w.name FROM workspaces w WHERE w.id = workspace_id) AS workspace_name,
               name, description, status, metadata, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(name)
        .bind(description)
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        row_to_initiative(&row)
    }

    pub(crate) async fn get_initiative_impl(&self, id: Uuid) -> Result<Initiative> {
        let row = sqlx::query(
            "SELECT i.id, i.workspace_id, w.name AS workspace_name,
                    i.name, i.description, i.status, i.metadata, i.created_at, i.updated_at
             FROM initiatives i
             JOIN workspaces w ON w.id = i.workspace_id
             WHERE i.id = $1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("initiative {id} not found")))?;
        row_to_initiative(&row)
    }

    pub(crate) async fn list_initiatives_by_workspace_impl(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<Initiative>> {
        let rows = sqlx::query(
            "SELECT i.id, i.workspace_id, w.name AS workspace_name,
                    i.name, i.description, i.status, i.metadata, i.created_at, i.updated_at
             FROM initiatives i
             JOIN workspaces w ON w.id = i.workspace_id
             WHERE i.workspace_id = $1
             ORDER BY i.name",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_initiative).collect()
    }

    pub(crate) async fn update_initiative_impl(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Initiative> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE initiatives
             SET name = $1, description = $2, status = $3, metadata = $4, updated_at = $5
             WHERE id = $6
             RETURNING id, workspace_id,
               (SELECT w.name FROM workspaces w WHERE w.id = workspace_id) AS workspace_name,
               name, description, status, metadata, created_at, updated_at",
        )
        .bind(name)
        .bind(description)
        .bind(status)
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("initiative {id} not found")))?;
        row_to_initiative(&row)
    }

    pub(crate) async fn delete_initiative_impl(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM initiatives WHERE id = $1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("initiative {id} not found")));
        }
        Ok(())
    }
}

fn row_to_initiative(row: &sqlx::sqlite::SqliteRow) -> Result<Initiative> {
    let id_str: String = row.try_get("id")?;
    let ws_id_str: String = row.try_get("workspace_id")?;
    let meta_str: Option<String> = row.try_get("metadata")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Initiative {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_uuid(&ws_id_str)?,
        workspace_name: row.try_get("workspace_name")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status: row.try_get("status")?,
        metadata: str_to_opt_value(meta_str),
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}
