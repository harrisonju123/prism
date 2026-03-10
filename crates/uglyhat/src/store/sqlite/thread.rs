use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn create_thread_impl(
        &self,
        workspace_id: Uuid,
        name: &str,
        desc: &str,
        tags: Vec<String>,
    ) -> Result<Thread> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO threads (id, workspace_id, name, description, status, tags, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'active', $5, $6, $7)
             RETURNING id, workspace_id, name, description, status, tags, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(name)
        .bind(desc)
        .bind(json_array_to_str(&tags))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint") {
                Error::Conflict(format!("thread {name:?} already exists"))
            } else {
                e.into()
            }
        })?;

        let thread = row_to_thread(&row)?;

        self.log_activity_fire_and_forget(
                workspace_id,
                "",
                "created",
                "thread",
                thread.id,
                &format!("Created thread: {name}"),
                None,
            )
            .await;

        Ok(thread)
    }

    pub(crate) async fn get_thread_impl(&self, workspace_id: Uuid, name: &str) -> Result<Thread> {
        let row = sqlx::query(
            "SELECT id, workspace_id, name, description, status, tags, created_at, updated_at
             FROM threads WHERE workspace_id = $1 AND name = $2",
        )
        .bind(workspace_id.to_string())
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("thread {name:?} not found")))?;
        row_to_thread(&row)
    }

    pub(crate) async fn list_threads_impl(
        &self,
        workspace_id: Uuid,
        status: Option<ThreadStatus>,
    ) -> Result<Vec<Thread>> {
        let rows = match status {
            Some(s) => sqlx::query(
                "SELECT id, workspace_id, name, description, status, tags, created_at, updated_at
                     FROM threads WHERE workspace_id = $1 AND status = $2
                     ORDER BY updated_at DESC",
            )
            .bind(workspace_id.to_string())
            .bind(s.to_string())
            .fetch_all(&self.pool)
            .await?,
            None => sqlx::query(
                "SELECT id, workspace_id, name, description, status, tags, created_at, updated_at
                     FROM threads WHERE workspace_id = $1
                     ORDER BY updated_at DESC",
            )
            .bind(workspace_id.to_string())
            .fetch_all(&self.pool)
            .await?,
        };
        rows.iter().map(row_to_thread).collect()
    }

    pub(crate) async fn archive_thread_impl(
        &self,
        workspace_id: Uuid,
        name: &str,
    ) -> Result<Thread> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE threads SET status = 'archived', updated_at = $1
             WHERE workspace_id = $2 AND name = $3
             RETURNING id, workspace_id, name, description, status, tags, created_at, updated_at",
        )
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("thread {name:?} not found")))?;

        let thread = row_to_thread(&row)?;

        self.log_activity_fire_and_forget(
                workspace_id,
                "",
                "archived",
                "thread",
                thread.id,
                &format!("Archived thread: {name}"),
                None,
            )
            .await;

        Ok(thread)
    }
}

pub(super) fn row_to_thread(row: &sqlx::sqlite::SqliteRow) -> Result<Thread> {
    let id_str: String = row.try_get("id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let status_str: String = row.try_get("status")?;
    let tags_str: String = row.try_get("tags")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    let status = match status_str.as_str() {
        "active" => ThreadStatus::Active,
        "archived" => ThreadStatus::Archived,
        other => {
            return Err(Error::Internal(format!("invalid thread status: {other}")));
        }
    };

    Ok(Thread {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status,
        tags: parse_json_array(&tags_str),
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}
