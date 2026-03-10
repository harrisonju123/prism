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

row_to_struct! {
    pub(super) fn row_to_workspace(row) -> Workspace {
        id: uuid "id",
        name: str "name",
        description: str "description",
        created_at: time "created_at",
        updated_at: time "updated_at",
    }
}
