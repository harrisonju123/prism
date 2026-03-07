use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn create_epic_impl(
        &self,
        initiative_id: Uuid,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Epic> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();

        // Look up workspace_id from initiative
        let ws_row = sqlx::query("SELECT workspace_id FROM initiatives WHERE id = $1")
            .bind(initiative_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound(format!("initiative {initiative_id} not found")))?;
        let ws_id: String = ws_row.try_get("workspace_id")?;

        let row = sqlx::query(
            "INSERT INTO epics (id, initiative_id, workspace_id, name, description, status, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, 'active', $6, $7, $8)
             RETURNING id, initiative_id,
               (SELECT i.name FROM initiatives i WHERE i.id = initiative_id) AS initiative_name,
               workspace_id,
               (SELECT w.name FROM workspaces w WHERE w.id = workspace_id) AS workspace_name,
               name, description, status, metadata, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(initiative_id.to_string())
        .bind(&ws_id)
        .bind(name)
        .bind(description)
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        row_to_epic(&row)
    }

    pub(crate) async fn get_epic_impl(&self, id: Uuid) -> Result<Epic> {
        let row = sqlx::query(
            "SELECT e.id, e.initiative_id, i.name AS initiative_name,
                    e.workspace_id, w.name AS workspace_name,
                    e.name, e.description, e.status, e.metadata, e.created_at, e.updated_at
             FROM epics e
             JOIN initiatives i ON i.id = e.initiative_id
             JOIN workspaces w ON w.id = e.workspace_id
             WHERE e.id = $1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("epic {id} not found")))?;
        row_to_epic(&row)
    }

    pub(crate) async fn list_epics_by_initiative_impl(
        &self,
        initiative_id: Uuid,
    ) -> Result<Vec<Epic>> {
        let rows = sqlx::query(
            "SELECT e.id, e.initiative_id, i.name AS initiative_name,
                    e.workspace_id, w.name AS workspace_name,
                    e.name, e.description, e.status, e.metadata, e.created_at, e.updated_at
             FROM epics e
             JOIN initiatives i ON i.id = e.initiative_id
             JOIN workspaces w ON w.id = e.workspace_id
             WHERE e.initiative_id = $1
             ORDER BY e.name",
        )
        .bind(initiative_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_epic).collect()
    }

    pub(crate) async fn update_epic_impl(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Epic> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE epics
             SET name = $1, description = $2, status = $3, metadata = $4, updated_at = $5
             WHERE id = $6
             RETURNING id, initiative_id,
               (SELECT i.name FROM initiatives i WHERE i.id = initiative_id) AS initiative_name,
               workspace_id,
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
        .ok_or_else(|| Error::NotFound(format!("epic {id} not found")))?;
        row_to_epic(&row)
    }

    pub(crate) async fn delete_epic_impl(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM epics WHERE id = $1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("epic {id} not found")));
        }
        Ok(())
    }
}

fn row_to_epic(row: &sqlx::sqlite::SqliteRow) -> Result<Epic> {
    let id_str: String = row.try_get("id")?;
    let init_id_str: String = row.try_get("initiative_id")?;
    let ws_id_str: String = row.try_get("workspace_id")?;
    let meta_str: Option<String> = row.try_get("metadata")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Epic {
        id: parse_uuid(&id_str)?,
        initiative_id: parse_uuid(&init_id_str)?,
        initiative_name: row.try_get("initiative_name")?,
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
