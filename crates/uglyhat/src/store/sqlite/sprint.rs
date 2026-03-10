use chrono::NaiveDate;
use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn create_sprint_impl(
        &self,
        workspace_id: Uuid,
        name: &str,
        goal: &str,
        start_date: Option<NaiveDate>,
        end_date: Option<NaiveDate>,
    ) -> Result<Sprint> {
        let id = Uuid::new_v4();
        let now = now_rfc3339();
        let start_str = start_date.map(|d| d.to_string());
        let end_str = end_date.map(|d| d.to_string());

        sqlx::query(
            "INSERT INTO sprints (id, workspace_id, name, goal, status, start_date, end_date, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8)",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(name)
        .bind(goal)
        .bind(&start_str)
        .bind(&end_str)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;

        self.get_sprint_impl(id).await
    }

    pub(crate) async fn list_sprints_impl(&self, workspace_id: Uuid) -> Result<Vec<Sprint>> {
        let rows = sqlx::query(
            "SELECT id, workspace_id, name, goal, status, start_date, end_date, created_at, updated_at
             FROM sprints WHERE workspace_id = $1 ORDER BY created_at DESC",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;

        rows.iter().map(|r| row_to_sprint(r)).collect()
    }

    pub(crate) async fn get_sprint_impl(&self, id: Uuid) -> Result<Sprint> {
        let row = sqlx::query(
            "SELECT id, workspace_id, name, goal, status, start_date, end_date, created_at, updated_at
             FROM sprints WHERE id = $1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Internal(e.to_string()))?
        .ok_or_else(|| Error::NotFound(id.to_string()))?;

        row_to_sprint(&row)
    }

    pub(crate) async fn close_sprint_impl(&self, id: Uuid) -> Result<Sprint> {
        let now = now_rfc3339();
        sqlx::query("UPDATE sprints SET status = 'closed', updated_at = $1 WHERE id = $2")
            .bind(&now)
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        self.get_sprint_impl(id).await
    }

    pub(crate) async fn assign_task_to_sprint_impl(
        &self,
        task_id: Uuid,
        sprint_id: Uuid,
    ) -> Result<()> {
        let id = Uuid::new_v4();
        let now = now_rfc3339();
        sqlx::query(
            "INSERT OR REPLACE INTO sprint_tasks (id, sprint_id, task_id, created_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id.to_string())
        .bind(sprint_id.to_string())
        .bind(task_id.to_string())
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(())
    }

    pub(crate) async fn sprint_velocity_impl(&self, sprint_id: Uuid) -> Result<SprintVelocity> {
        let sprint = self.get_sprint_impl(sprint_id).await?;

        let row = sqlx::query(
            "SELECT
                COUNT(*) as total,
                SUM(CASE WHEN t.status = 'done' THEN 1 ELSE 0 END) as done
             FROM sprint_tasks st
             JOIN tasks t ON t.id = st.task_id
             WHERE st.sprint_id = $1",
        )
        .bind(sprint_id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;

        let total: i64 = row.try_get("total").unwrap_or(0);
        let done: i64 = row.try_get("done").unwrap_or(0);
        let completion_pct = if total > 0 {
            (done as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        Ok(SprintVelocity {
            sprint_id,
            sprint_name: sprint.name,
            total_tasks: total,
            done_tasks: done,
            completion_pct,
        })
    }

    pub(crate) async fn upsert_task_by_github_id_impl(
        &self,
        workspace_id: Uuid,
        epic_id: Uuid,
        github_issue_id: i64,
        name: &str,
        description: &str,
    ) -> Result<Task> {
        let now = now_rfc3339();

        let existing =
            sqlx::query("SELECT id FROM tasks WHERE workspace_id = $1 AND metadata LIKE $2")
                .bind(workspace_id.to_string())
                .bind(format!("%\"github_issue_id\":{github_issue_id}%"))
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;

        if let Some(row) = existing {
            let id_str: String = row
                .try_get("id")
                .map_err(|e| Error::Internal(e.to_string()))?;
            let id = parse_uuid(&id_str)?;
            sqlx::query(
                "UPDATE tasks SET name = $1, description = $2, updated_at = $3 WHERE id = $4",
            )
            .bind(name)
            .bind(description)
            .bind(&now)
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

            return self.get_task_impl(id).await;
        }

        let epic_row = sqlx::query("SELECT initiative_id FROM epics WHERE id = $1")
            .bind(epic_id.to_string())
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        let initiative_id_str: String = epic_row
            .try_get("initiative_id")
            .map_err(|e| Error::Internal(e.to_string()))?;
        let initiative_id = parse_uuid(&initiative_id_str)?;

        let id = Uuid::new_v4();
        let metadata = serde_json::json!({"github_issue_id": github_issue_id});

        sqlx::query(
            "INSERT INTO tasks (id, epic_id, initiative_id, workspace_id, name, description, status, priority, assignee, domain_tags, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'backlog', 'medium', '', '[]', $7, $8, $9)",
        )
        .bind(id.to_string())
        .bind(epic_id.to_string())
        .bind(initiative_id.to_string())
        .bind(workspace_id.to_string())
        .bind(name)
        .bind(description)
        .bind(metadata.to_string())
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;

        self.get_task_impl(id).await
    }
}

fn row_to_sprint(row: &sqlx::sqlite::SqliteRow) -> Result<Sprint> {
    let id = parse_uuid(
        &row.try_get::<String, _>("id")
            .map_err(|e| Error::Internal(e.to_string()))?,
    )?;
    let workspace_id = parse_uuid(
        &row.try_get::<String, _>("workspace_id")
            .map_err(|e| Error::Internal(e.to_string()))?,
    )?;
    let start_date: Option<NaiveDate> = row
        .try_get::<Option<String>, _>("start_date")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok());
    let end_date: Option<NaiveDate> = row
        .try_get::<Option<String>, _>("end_date")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok());

    Ok(Sprint {
        id,
        workspace_id,
        name: row
            .try_get("name")
            .map_err(|e| Error::Internal(e.to_string()))?,
        goal: row.try_get::<String, _>("goal").unwrap_or_default(),
        status: row
            .try_get("status")
            .map_err(|e| Error::Internal(e.to_string()))?,
        start_date,
        end_date,
        created_at: parse_time(
            &row.try_get::<String, _>("created_at")
                .map_err(|e| Error::Internal(e.to_string()))?,
        )?,
        updated_at: parse_time(
            &row.try_get::<String, _>("updated_at")
                .map_err(|e| Error::Internal(e.to_string()))?,
        )?,
    })
}
