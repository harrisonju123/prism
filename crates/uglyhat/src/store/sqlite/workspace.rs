use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;
use crate::store::BootstrapResult;

impl SqliteStore {
    pub(crate) async fn bootstrap_workspace_impl(
        &self,
        name: &str,
        description: &str,
        key_hash: &str,
        key_prefix: &str,
    ) -> Result<BootstrapResult> {
        let mut tx = self.pool.begin().await?;
        let now = now_rfc3339();

        // 1. Create workspace
        let w_id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO workspaces (id, name, description, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, NULL, $4, $5)
             RETURNING id, name, description, metadata, created_at, updated_at",
        )
        .bind(w_id.to_string())
        .bind(name)
        .bind(description)
        .bind(&now)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await?;

        let mut w = row_to_workspace(&row)?;

        // 2. Create "System" initiative
        let init_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO initiatives (id, workspace_id, name, description, status, metadata, created_at, updated_at)
             VALUES ($1, $2, 'System', 'Auto-created system initiative', 'active', NULL, $3, $4)",
        )
        .bind(init_id.to_string())
        .bind(w.id.to_string())
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        // 3. Create "Reported Issues" epic
        let epic_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO epics (id, initiative_id, workspace_id, name, description, status, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, 'Reported Issues', 'Agent-reported issues', 'active', NULL, $4, $5)",
        )
        .bind(epic_id.to_string())
        .bind(init_id.to_string())
        .bind(w.id.to_string())
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        // 4. Store system IDs in workspace metadata
        let meta = serde_json::json!({
            "system_initiative_id": init_id.to_string(),
            "system_epic_id": epic_id.to_string(),
        });
        let now2 = now_rfc3339();
        let row = sqlx::query(
            "UPDATE workspaces SET metadata = $1, updated_at = $2 WHERE id = $3
             RETURNING id, name, description, metadata, created_at, updated_at",
        )
        .bind(serde_json::to_string(&meta).unwrap())
        .bind(&now2)
        .bind(w.id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        w = row_to_workspace(&row)?;

        // 5. Create default API key
        let k_id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO api_keys (id, workspace_id, name, key_hash, key_prefix, created_at)
             VALUES ($1, $2, 'default', $3, $4, $5)
             RETURNING id, workspace_id, name, key_hash, key_prefix, created_at",
        )
        .bind(k_id.to_string())
        .bind(w.id.to_string())
        .bind(key_hash)
        .bind(key_prefix)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await?;
        let api_key = super::apikey::row_to_api_key(&row)?;

        tx.commit().await?;

        Ok(BootstrapResult {
            workspace: w,
            initiative_id: init_id,
            epic_id,
            api_key,
        })
    }

    pub(crate) async fn get_system_epic_id_impl(&self, workspace_id: Uuid) -> Result<Uuid> {
        let row = sqlx::query("SELECT metadata FROM workspaces WHERE id = $1")
            .bind(workspace_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workspace {workspace_id} not found")))?;

        let meta_str: Option<String> = row.try_get("metadata")?;
        let meta_str = meta_str
            .ok_or_else(|| Error::Internal(format!("workspace {workspace_id} has no metadata")))?;

        let meta: std::collections::HashMap<String, String> = serde_json::from_str(&meta_str)
            .map_err(|e| Error::Internal(format!("parse workspace metadata: {e}")))?;

        let epic_str = meta.get("system_epic_id").ok_or_else(|| {
            Error::Internal(format!(
                "workspace {workspace_id} has no system_epic_id in metadata"
            ))
        })?;

        parse_uuid(epic_str)
    }

    pub(crate) async fn create_workspace_impl(
        &self,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Workspace> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO workspaces (id, name, description, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, name, description, metadata, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(description)
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        row_to_workspace(&row)
    }

    pub(crate) async fn get_workspace_impl(&self, id: Uuid) -> Result<Workspace> {
        let row = sqlx::query(
            "SELECT id, name, description, metadata, created_at, updated_at
             FROM workspaces WHERE id = $1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("workspace {id} not found")))?;
        row_to_workspace(&row)
    }

    pub(crate) async fn list_workspaces_impl(&self) -> Result<Vec<Workspace>> {
        let rows = sqlx::query(
            "SELECT id, name, description, metadata, created_at, updated_at
             FROM workspaces ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_workspace).collect()
    }

    pub(crate) async fn update_workspace_impl(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Workspace> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE workspaces
             SET name = $1, description = $2, metadata = $3, updated_at = $4
             WHERE id = $5
             RETURNING id, name, description, metadata, created_at, updated_at",
        )
        .bind(name)
        .bind(description)
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("workspace {id} not found")))?;
        row_to_workspace(&row)
    }

    pub(crate) async fn delete_workspace_impl(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM workspaces WHERE id = $1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("workspace {id} not found")));
        }
        Ok(())
    }
}

fn row_to_workspace(row: &sqlx::sqlite::SqliteRow) -> Result<Workspace> {
    let id_str: String = row.try_get("id")?;
    let meta_str: Option<String> = row.try_get("metadata")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Workspace {
        id: parse_uuid(&id_str)?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        metadata: str_to_opt_value(meta_str),
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}
