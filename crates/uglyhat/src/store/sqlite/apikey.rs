use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn create_api_key_impl(
        &self,
        workspace_id: Uuid,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
    ) -> Result<APIKey> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO api_keys (id, workspace_id, name, key_hash, key_prefix, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, workspace_id, name, key_hash, key_prefix, created_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(name)
        .bind(key_hash)
        .bind(key_prefix)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        row_to_api_key(&row)
    }

    pub(crate) async fn get_api_key_by_hash_impl(&self, key_hash: &str) -> Result<APIKey> {
        let row = sqlx::query(
            "SELECT id, workspace_id, name, key_hash, key_prefix, created_at
             FROM api_keys WHERE key_hash = $1",
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound("api key not found".to_string()))?;
        row_to_api_key(&row)
    }

    pub(crate) async fn list_api_keys_by_workspace_impl(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<APIKey>> {
        let rows = sqlx::query(
            "SELECT id, workspace_id, name, key_hash, key_prefix, created_at
             FROM api_keys WHERE workspace_id = $1 ORDER BY created_at DESC",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_api_key).collect()
    }

    pub(crate) async fn delete_api_key_impl(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM api_keys WHERE id = $1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("api key {id} not found")));
        }
        Ok(())
    }
}

pub(super) fn row_to_api_key(row: &sqlx::sqlite::SqliteRow) -> Result<APIKey> {
    let id_str: String = row.try_get("id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let created_str: String = row.try_get("created_at")?;

    Ok(APIKey {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        name: row.try_get("name")?,
        key_hash: row.try_get("key_hash")?,
        key_prefix: row.try_get("key_prefix")?,
        created_at: parse_time(&created_str)?,
    })
}
