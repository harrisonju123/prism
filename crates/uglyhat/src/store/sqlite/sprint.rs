use chrono::{NaiveDate, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::{now_rfc3339, parse_time, parse_uuid};
use crate::error::{Error, Result};
use crate::model::{Sprint, SprintVelocity, Task};

impl SqliteStore {
    pub(super) async fn create_sprint_impl(
        &self,
        workspace_id: Uuid,
        name: &str,
        goal: &str,
        start_date: Option<NaiveDate>,
        end_date: Option<NaiveDate>,
    ) -> Result<Sprint> {
        let id = Uuid::new_v4();
        let now_str = now_rfc3339();
        let ws_str = workspace_id.to_string();
        let id_str = id.to_string();
        let start_str = start_date.map(|d| d.format("%Y-%m-%d").to_string());
        let end_str = end_date.map(|d| d.format("%Y-%m-%d").to_string());

        sqlx::query(
            "INSERT INTO sprints (id, workspace_id, name, goal, start_date, end_date, status, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'active', $7, $8)",
        )
        .bind(&id_str)
        .bind(&ws_str)
        .bind(name)
        .bind(goal)
        .bind(&start_str)
        .bind(&end_str)
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("create_sprint: {e}")))?;

        self.get_sprint_impl(id).await
    }

    pub(super) async fn get_sprint_impl(&self, id: Uuid) -> Result<Sprint> {
        let id_str = id.to_string();
        let row = sqlx::query(
            "SELECT id, workspace_id, name, goal, start_date, end_date, status, created_at, updated_at FROM sprints WHERE id = $1",
        )
        .bind(&id_str)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("get_sprint: {e}")))?
        .ok_or_else(|| Error::NotFound(format!("sprint {id} not found")))?;

        row_to_sprint(&row)
    }

    pub(super) async fn list_sprints_impl(&self, workspace_id: Uuid) -> Result<Vec<Sprint>> {
        let ws_str = workspace_id.to_string();
        let rows = sqlx::query(
            "SELECT id, workspace_id, name, goal, start_date, end_date, status, created_at, updated_at FROM sprints WHERE workspace_id = $1 ORDER BY created_at DESC",
        )
        .bind(&ws_str)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("list_sprints: {e}")))?;

        rows.iter().map(row_to_sprint).collect()
    }

    pub(super) async fn close_sprint_impl(&self, id: Uuid) -> Result<Sprint> {
        let id_str = id.to_string();
        let now_str = now_rfc3339();
        sqlx::query("UPDATE sprints SET status = 'closed', updated_at = $1 WHERE id = $2")
            .bind(&now_str)
            .bind(&id_str)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("close_sprint: {e}")))?;
        self.get_sprint_impl(id).await
    }

    pub(super) async fn assign_task_to_sprint_impl(
        &self,
        task_id: Uuid,
        sprint_id: Uuid,
    ) -> Result<()> {
        let task_id_str = task_id.to_string();
        let sprint_id_str = sprint_id.to_string();
        let now_str = now_rfc3339();
        sqlx::query("UPDATE tasks SET sprint_id = $1, updated_at = $2 WHERE id = $3")
            .bind(&sprint_id_str)
            .bind(&now_str)
            .bind(&task_id_str)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("assign_task_to_sprint: {e}")))?;
        Ok(())
    }

    pub(super) async fn sprint_velocity_impl(&self, sprint_id: Uuid) -> Result<SprintVelocity> {
        let sprint = self.get_sprint_impl(sprint_id).await?;
        let sprint_id_str = sprint_id.to_string();

        let row = sqlx::query(
            "SELECT COUNT(*) AS total, COUNT(CASE WHEN status = 'done' THEN 1 END) AS done \
             FROM tasks WHERE sprint_id = $1",
        )
        .bind(&sprint_id_str)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("sprint_velocity: {e}")))?;

        let total_tasks: i64 = row.try_get("total")?;
        let done_tasks: i64 = row.try_get("done")?;

        let remaining = total_tasks - done_tasks;
        let velocity = if total_tasks > 0 {
            done_tasks as f64 / total_tasks as f64
        } else {
            0.0
        };

        let days_left = sprint
            .end_date
            .map(|end| {
                let today = Utc::now().date_naive();
                (end - today).num_days().max(0)
            })
            .unwrap_or(0);

        Ok(SprintVelocity {
            sprint,
            total_tasks,
            done_tasks,
            remaining,
            velocity,
            days_left,
        })
    }

    pub(super) async fn upsert_task_by_github_id_impl(
        &self,
        workspace_id: Uuid,
        epic_id: Uuid,
        github_issue_id: i64,
        name: &str,
        description: &str,
    ) -> Result<Task> {
        let ws_str = workspace_id.to_string();
        let github_id_str = github_issue_id.to_string();

        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM tasks WHERE workspace_id = $1 AND github_issue_id = $2",
        )
        .bind(&ws_str)
        .bind(&github_id_str)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("check github task: {e}")))?;

        if let Some(task_id_str) = existing {
            let task_id = task_id_str
                .parse::<Uuid>()
                .map_err(|e| Error::Internal(e.to_string()))?;
            let now_str = now_rfc3339();
            sqlx::query(
                "UPDATE tasks SET name = $1, description = $2, updated_at = $3 WHERE id = $4",
            )
            .bind(name)
            .bind(description)
            .bind(&now_str)
            .bind(&task_id_str)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("update github task: {e}")))?;
            self.get_task_impl(task_id).await
        } else {
            let id = Uuid::new_v4();
            let now_str = now_rfc3339();
            let id_str = id.to_string();
            let epic_str = epic_id.to_string();

            let initiative_id_str: String =
                sqlx::query_scalar("SELECT initiative_id FROM epics WHERE id = $1")
                    .bind(&epic_str)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| Error::Internal(format!("get initiative from epic: {e}")))?
                    .ok_or_else(|| Error::NotFound(format!("epic {epic_id} not found")))?;

            sqlx::query(
                "INSERT INTO tasks (id, epic_id, initiative_id, workspace_id, name, description, status, priority, assignee, domain_tags, github_issue_id, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, 'todo', 'medium', '', '[]', $7, $8, $9)",
            )
            .bind(&id_str)
            .bind(&epic_str)
            .bind(&initiative_id_str)
            .bind(&ws_str)
            .bind(name)
            .bind(description)
            .bind(&github_id_str)
            .bind(&now_str)
            .bind(&now_str)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("create github task: {e}")))?;

            self.get_task_impl(id).await
        }
    }
}

fn row_to_sprint(row: &sqlx::sqlite::SqliteRow) -> Result<Sprint> {
    let id_str: String = row.try_get("id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let start_str: Option<String> = row.try_get("start_date")?;
    let end_str: Option<String> = row.try_get("end_date")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Sprint {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        name: row.try_get("name")?,
        goal: row
            .try_get::<Option<String>, _>("goal")?
            .unwrap_or_default(),
        start_date: start_str
            .as_deref()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()),
        end_date: end_str
            .as_deref()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()),
        status: row.try_get("status")?,
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}
