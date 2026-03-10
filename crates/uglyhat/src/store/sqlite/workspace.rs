use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn init_workspace_impl(&self, name: &str, desc: &str) -> Result<Workspace> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO workspaces (id, name, description, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, name, description, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(desc)
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        row_to_workspace(&row)
    }

    pub(crate) async fn get_workspace_impl(&self, id: Uuid) -> Result<Workspace> {
        let row = sqlx::query(
            "SELECT id, name, description, created_at, updated_at
             FROM workspaces WHERE id = $1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("workspace {id} not found")))?;
        row_to_workspace(&row)
    }
}

pub(super) fn row_to_workspace(row: &sqlx::sqlite::SqliteRow) -> Result<Workspace> {
    let id_str: String = row.try_get("id")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Workspace {
        id: parse_uuid(&id_str)?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}
