use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;
use crate::store::TaskFilters;

const TASK_SELECT_COLS: &str = "t.id, t.epic_id, ep.name AS epic_name, t.initiative_id, i.name AS initiative_name, t.workspace_id, w.name AS workspace_name,
       t.name, t.description, t.status, t.priority, t.assignee, t.domain_tags,
       t.metadata, t.created_at, t.updated_at";

const TASK_JOINS: &str = "FROM tasks t
JOIN epics ep ON ep.id = t.epic_id
JOIN initiatives i ON i.id = t.initiative_id
JOIN workspaces w ON w.id = t.workspace_id";

impl SqliteStore {
    pub(crate) async fn create_task_impl(
        &self,
        epic_id: Uuid,
        name: &str,
        description: &str,
        status: TaskStatus,
        priority: TaskPriority,
        assignee: &str,
        domain_tags: Vec<String>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Task> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();

        // Look up initiative_id and workspace_id from epic
        let epic_row = sqlx::query("SELECT initiative_id, workspace_id FROM epics WHERE id = $1")
            .bind(epic_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound(format!("epic {epic_id} not found")))?;
        let init_id_str: String = epic_row.try_get("initiative_id")?;
        let ws_id_str: String = epic_row.try_get("workspace_id")?;

        sqlx::query(
            "INSERT INTO tasks (id, epic_id, initiative_id, workspace_id, name, description, status, priority, assignee, domain_tags, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(id.to_string())
        .bind(epic_id.to_string())
        .bind(&init_id_str)
        .bind(&ws_id_str)
        .bind(name)
        .bind(description)
        .bind(status.to_string())
        .bind(priority.to_string())
        .bind(assignee)
        .bind(json_array_to_str(&domain_tags))
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        let t = self.fetch_task_by_id(&id.to_string()).await?;

        let ws_id = parse_uuid(&ws_id_str)?;
        let _ = self
            .log_activity_impl(
                ws_id,
                "",
                "created",
                "task",
                t.id,
                &format!("Created task: {}", t.name),
                None,
            )
            .await;
        Ok(t)
    }

    pub(crate) async fn get_task_impl(&self, id: Uuid) -> Result<Task> {
        let query = format!("SELECT {TASK_SELECT_COLS} {TASK_JOINS} WHERE t.id = $1");
        let row = sqlx::query(&query)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound(format!("task {id} not found")))?;
        row_to_task(&row)
    }

    pub(crate) async fn update_task_impl(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        status: TaskStatus,
        priority: TaskPriority,
        assignee: &str,
        domain_tags: Vec<String>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Task> {
        let now = now_rfc3339();
        let result = sqlx::query(
            "UPDATE tasks
             SET name = $1, description = $2, status = $3, priority = $4, assignee = $5,
                 domain_tags = $6, metadata = $7, updated_at = $8
             WHERE id = $9",
        )
        .bind(name)
        .bind(description)
        .bind(status.to_string())
        .bind(priority.to_string())
        .bind(assignee)
        .bind(json_array_to_str(&domain_tags))
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("task {id} not found")));
        }

        let t = self.fetch_task_by_id(&id.to_string()).await?;

        let _ = self
            .log_activity_impl(
                t.workspace_id,
                "",
                "updated",
                "task",
                t.id,
                &format!("Updated task: {}", t.name),
                None,
            )
            .await;

        // Upward propagation: auto-close epic if all tasks are done/cancelled
        if t.status.is_terminal() {
            self.maybe_close_epic(t.epic_id, t.initiative_id).await;
        }

        Ok(t)
    }

    async fn maybe_close_epic(&self, epic_id: Uuid, initiative_id: Uuid) {
        let count_result = sqlx::query(
            "SELECT COUNT(*) AS cnt FROM tasks WHERE epic_id = $1 AND status NOT IN ('done', 'cancelled')",
        )
        .bind(epic_id.to_string())
        .fetch_one(&self.pool)
        .await;

        if let Ok(row) = count_result {
            let count: i64 = row.try_get("cnt").unwrap_or(1);
            if count > 0 {
                return;
            }
        } else {
            return;
        }

        if let Err(e) = sqlx::query(
            "UPDATE epics SET status = 'done', updated_at = $1 WHERE id = $2 AND status != 'done'",
        )
        .bind(now_rfc3339())
        .bind(epic_id.to_string())
        .execute(&self.pool)
        .await
        {
            tracing::warn!(epic_id = %epic_id, error = %e, "failed to auto-close epic");
        }

        self.maybe_close_initiative(initiative_id).await;
    }

    async fn maybe_close_initiative(&self, initiative_id: Uuid) {
        let count_result = sqlx::query(
            "SELECT COUNT(*) AS cnt FROM epics WHERE initiative_id = $1 AND status NOT IN ('done', 'cancelled')",
        )
        .bind(initiative_id.to_string())
        .fetch_one(&self.pool)
        .await;

        if let Ok(row) = count_result {
            let count: i64 = row.try_get("cnt").unwrap_or(1);
            if count > 0 {
                return;
            }
        } else {
            return;
        }

        if let Err(e) = sqlx::query(
            "UPDATE initiatives SET status = 'done', updated_at = $1 WHERE id = $2 AND status != 'done'",
        )
        .bind(now_rfc3339())
        .bind(initiative_id.to_string())
        .execute(&self.pool)
        .await
        {
            tracing::warn!(initiative_id = %initiative_id, error = %e, "failed to auto-close initiative");
        }
    }

    pub(crate) async fn delete_task_impl(&self, id: Uuid) -> Result<()> {
        let info_row = sqlx::query("SELECT workspace_id, name FROM tasks WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound(format!("task {id} not found")))?;

        let ws_id_str: String = info_row.try_get("workspace_id")?;
        let task_name: String = info_row.try_get("name")?;

        let result = sqlx::query("DELETE FROM tasks WHERE id = $1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("task {id} not found")));
        }

        let ws_id = parse_uuid(&ws_id_str)?;
        let _ = self
            .log_activity_impl(
                ws_id,
                "",
                "deleted",
                "task",
                id,
                &format!("Deleted task: {task_name}"),
                None,
            )
            .await;
        Ok(())
    }

    pub(crate) async fn list_tasks_by_epic_impl(&self, epic_id: Uuid) -> Result<Vec<Task>> {
        let query = format!(
            "SELECT {TASK_SELECT_COLS} {TASK_JOINS} WHERE t.epic_id = $1 ORDER BY t.created_at"
        );
        let rows = sqlx::query(&query)
            .bind(epic_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_task).collect()
    }

    pub(crate) async fn list_tasks_by_workspace_impl(
        &self,
        workspace_id: Uuid,
        filters: TaskFilters,
    ) -> Result<Vec<Task>> {
        let mut clauses = vec!["t.workspace_id = $1".to_string()];
        let mut args: Vec<String> = vec![workspace_id.to_string()];

        if let Some(ref status) = filters.status {
            args.push(status.to_string());
            clauses.push(format!("t.status = ${}", args.len()));
        }
        if let Some(ref priority) = filters.priority {
            args.push(priority.to_string());
            clauses.push(format!("t.priority = ${}", args.len()));
        }
        if let Some(ref domain) = filters.domain {
            args.push(domain.clone());
            clauses.push(format!(
                "EXISTS (SELECT 1 FROM json_each(t.domain_tags) WHERE json_each.value = ${})",
                args.len()
            ));
        }
        if let Some(ref assignee) = filters.assignee {
            args.push(assignee.clone());
            clauses.push(format!("t.assignee = ${}", args.len()));
        }
        if let Some(true) = filters.unassigned {
            clauses.push("t.assignee = ''".to_string());
        }

        let where_clause = clauses.join(" AND ");
        let query = format!(
            "SELECT {TASK_SELECT_COLS} {TASK_JOINS} WHERE {where_clause} ORDER BY t.created_at"
        );

        // Build query dynamically with bind
        let mut q = sqlx::query(&query);
        for arg in &args {
            q = q.bind(arg);
        }

        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_task).collect()
    }

    pub(crate) async fn claim_task_impl(
        &self,
        workspace_id: Uuid,
        task_id: Uuid,
        agent_name: &str,
    ) -> Result<Task> {
        let now = now_rfc3339();
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            "UPDATE tasks SET assignee = $1, status = 'in_progress', updated_at = $2
             WHERE id = $3 AND workspace_id = $4",
        )
        .bind(agent_name)
        .bind(&now)
        .bind(task_id.to_string())
        .bind(workspace_id.to_string())
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("task {task_id} not found")));
        }

        // Best-effort: update agent's current_task_id
        let _ = sqlx::query(
            "UPDATE agents SET current_task_id = $1, updated_at = $2
             WHERE workspace_id = $3 AND name = $4",
        )
        .bind(task_id.to_string())
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(agent_name)
        .execute(&mut *tx)
        .await;

        tx.commit().await?;

        self.get_task_impl(task_id).await
    }

    pub(crate) async fn fetch_task_by_id(&self, id: &str) -> Result<Task> {
        let query = format!("SELECT {TASK_SELECT_COLS} {TASK_JOINS} WHERE t.id = $1");
        let row = sqlx::query(&query).bind(id).fetch_one(&self.pool).await?;
        row_to_task(&row)
    }

    pub(crate) async fn scan_task_summaries(
        &self,
        query: &str,
        args: Vec<String>,
    ) -> Result<Vec<TaskSummary>> {
        let mut q = sqlx::query(query);
        for arg in &args {
            q = q.bind(arg);
        }
        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_task_summary).collect()
    }
}

pub(crate) fn row_to_task(row: &sqlx::sqlite::SqliteRow) -> Result<Task> {
    let id_str: String = row.try_get("id")?;
    let epic_id_str: String = row.try_get("epic_id")?;
    let init_id_str: String = row.try_get("initiative_id")?;
    let ws_id_str: String = row.try_get("workspace_id")?;
    let status_str: String = row.try_get("status")?;
    let priority_str: String = row.try_get("priority")?;
    let tags_str: String = row.try_get("domain_tags")?;
    let meta_str: Option<String> = row.try_get("metadata")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    let status: TaskStatus = serde_json::from_value(serde_json::Value::String(status_str))
        .map_err(|e| Error::Internal(format!("invalid task status: {e}")))?;
    let priority: TaskPriority = serde_json::from_value(serde_json::Value::String(priority_str))
        .map_err(|e| Error::Internal(format!("invalid task priority: {e}")))?;

    Ok(Task {
        id: parse_uuid(&id_str)?,
        epic_id: parse_uuid(&epic_id_str)?,
        epic_name: row.try_get("epic_name")?,
        initiative_id: parse_uuid(&init_id_str)?,
        initiative_name: row.try_get("initiative_name")?,
        workspace_id: parse_uuid(&ws_id_str)?,
        workspace_name: row.try_get("workspace_name")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status,
        priority,
        assignee: row.try_get("assignee")?,
        domain_tags: parse_json_array(&tags_str),
        metadata: str_to_opt_value(meta_str),
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
        blocks: vec![],
        blocked_by: vec![],
    })
}

pub(crate) fn row_to_task_summary(row: &sqlx::sqlite::SqliteRow) -> Result<TaskSummary> {
    let id_str: String = row.try_get("id")?;
    let status_str: String = row.try_get("status")?;
    let priority_str: String = row.try_get("priority")?;
    let tags_str: String = row.try_get("domain_tags")?;
    let created_str: String = row.try_get("created_at")?;

    let status: TaskStatus = serde_json::from_value(serde_json::Value::String(status_str))
        .map_err(|e| Error::Internal(format!("invalid task status: {e}")))?;
    let priority: TaskPriority = serde_json::from_value(serde_json::Value::String(priority_str))
        .map_err(|e| Error::Internal(format!("invalid task priority: {e}")))?;

    Ok(TaskSummary {
        id: parse_uuid(&id_str)?,
        name: row.try_get("name")?,
        status,
        priority,
        assignee: row.try_get("assignee")?,
        epic_name: row.try_get("epic_name")?,
        initiative_name: row.try_get("initiative_name")?,
        domain_tags: parse_json_array(&tags_str),
        created_at: parse_time(&created_str)?,
    })
}
