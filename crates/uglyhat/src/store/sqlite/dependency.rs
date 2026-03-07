use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn add_dependency_impl(
        &self,
        blocking_task_id: Uuid,
        blocked_task_id: Uuid,
    ) -> Result<TaskDependency> {
        // Verify both tasks exist and are in the same workspace
        let ws_row = sqlx::query(
            "SELECT t1.workspace_id FROM tasks t1
             JOIN tasks t2 ON t2.workspace_id = t1.workspace_id
             WHERE t1.id = $1 AND t2.id = $2",
        )
        .bind(blocking_task_id.to_string())
        .bind(blocked_task_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| {
            Error::NotFound(format!(
                "tasks {blocking_task_id} and/or {blocked_task_id} not found"
            ))
        })?;
        let ws_id_str: String = ws_row.try_get("workspace_id")?;

        // Check for cycles using recursive CTE
        let cycle_row = sqlx::query(
            "WITH RECURSIVE dep_chain AS (
                SELECT blocking_task_id AS task_id FROM task_dependencies WHERE blocked_task_id = $1
                UNION
                SELECT td.blocking_task_id FROM task_dependencies td
                JOIN dep_chain dc ON dc.task_id = td.blocked_task_id
            )
            SELECT EXISTS(SELECT 1 FROM dep_chain WHERE task_id = $2) AS has_cycle",
        )
        .bind(blocking_task_id.to_string())
        .bind(blocked_task_id.to_string())
        .fetch_one(&self.pool)
        .await?;
        let has_cycle: bool = cycle_row.try_get("has_cycle")?;
        if has_cycle {
            return Err(Error::Conflict(format!(
                "adding dependency from {blocking_task_id} to {blocked_task_id} would create a cycle"
            )));
        }

        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO task_dependencies (id, blocking_task_id, blocked_task_id, workspace_id, created_at)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, blocking_task_id, blocked_task_id, workspace_id, created_at",
        )
        .bind(id.to_string())
        .bind(blocking_task_id.to_string())
        .bind(blocked_task_id.to_string())
        .bind(&ws_id_str)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        let dep_id_str: String = row.try_get("id")?;
        let blocking_str: String = row.try_get("blocking_task_id")?;
        let blocked_str: String = row.try_get("blocked_task_id")?;
        let dep_ws_str: String = row.try_get("workspace_id")?;
        let created_str: String = row.try_get("created_at")?;

        Ok(TaskDependency {
            id: parse_uuid(&dep_id_str)?,
            blocking_task_id: parse_uuid(&blocking_str)?,
            blocked_task_id: parse_uuid(&blocked_str)?,
            workspace_id: parse_uuid(&dep_ws_str)?,
            created_at: parse_time(&created_str)?,
        })
    }

    pub(crate) async fn remove_dependency_impl(&self, dep_id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM task_dependencies WHERE id = $1")
            .bind(dep_id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("dependency {dep_id} not found")));
        }
        Ok(())
    }

    pub(crate) async fn get_dependencies_impl(
        &self,
        task_id: Uuid,
    ) -> Result<(Vec<DependencyInfo>, Vec<DependencyInfo>)> {
        // Tasks this task blocks
        let block_rows = sqlx::query(
            "SELECT td.blocked_task_id AS task_id, t.name AS task_name, t.status
             FROM task_dependencies td
             JOIN tasks t ON t.id = td.blocked_task_id
             WHERE td.blocking_task_id = $1
             ORDER BY t.name",
        )
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let blocks: Vec<DependencyInfo> = block_rows
            .iter()
            .map(row_to_dependency_info)
            .collect::<Result<_>>()?;

        // Tasks that block this task
        let blocked_by_rows = sqlx::query(
            "SELECT td.blocking_task_id AS task_id, t.name AS task_name, t.status
             FROM task_dependencies td
             JOIN tasks t ON t.id = td.blocking_task_id
             WHERE td.blocked_task_id = $1
             ORDER BY t.name",
        )
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let blocked_by: Vec<DependencyInfo> = blocked_by_rows
            .iter()
            .map(row_to_dependency_info)
            .collect::<Result<_>>()?;

        Ok((blocks, blocked_by))
    }
}

fn row_to_dependency_info(row: &sqlx::sqlite::SqliteRow) -> Result<DependencyInfo> {
    let id_str: String = row.try_get("task_id")?;
    let status_str: String = row.try_get("status")?;
    let status: TaskStatus = serde_json::from_value(serde_json::Value::String(status_str))
        .map_err(|e| Error::Internal(format!("invalid task status: {e}")))?;

    Ok(DependencyInfo {
        task_id: parse_uuid(&id_str)?,
        task_name: row.try_get("task_name")?,
        status,
    })
}
