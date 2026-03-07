use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::Result;
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn get_workspace_context_impl(
        &self,
        workspace_id: Uuid,
    ) -> Result<WorkspaceContext> {
        let ws_str = workspace_id.to_string();

        let (
            workspace,
            initiatives,
            active_tasks,
            recent_tasks,
            decisions,
            by_status,
            by_priority,
            blocked_tasks_count,
        ) = tokio::try_join!(
            self.get_workspace_impl(workspace_id),
            self.fetch_initiatives_with_counts(&ws_str),
            self.scan_task_summaries(
                "SELECT t.id, t.name, t.status, t.priority, t.assignee,
                        ep.name AS epic_name, i.name AS initiative_name, t.domain_tags, t.created_at
                 FROM tasks t
                 JOIN epics ep ON ep.id = t.epic_id
                 JOIN initiatives i ON i.id = t.initiative_id
                 WHERE t.workspace_id = $1
                   AND t.status IN ('in_progress', 'in_review')
                 ORDER BY t.updated_at DESC
                 LIMIT 20",
                vec![ws_str.clone()],
            ),
            self.scan_task_summaries(
                "SELECT t.id, t.name, t.status, t.priority, t.assignee,
                        ep.name AS epic_name, i.name AS initiative_name, t.domain_tags, t.created_at
                 FROM tasks t
                 JOIN epics ep ON ep.id = t.epic_id
                 JOIN initiatives i ON i.id = t.initiative_id
                 WHERE t.workspace_id = $1
                 ORDER BY t.updated_at DESC
                 LIMIT 10",
                vec![ws_str.clone()],
            ),
            self.fetch_recent_decisions(&ws_str),
            self.fetch_status_counts(&ws_str),
            self.fetch_priority_counts(&ws_str),
            self.fetch_blocked_count(&ws_str),
        )?;

        Ok(WorkspaceContext {
            workspace,
            initiatives,
            active_tasks,
            recent_tasks,
            decisions,
            tasks_by_status: by_status,
            tasks_by_priority: by_priority,
            blocked_tasks_count,
        })
    }

    async fn fetch_initiatives_with_counts(
        &self,
        ws_str: &str,
    ) -> Result<Vec<InitiativeWithCounts>> {
        let rows = sqlx::query(
            "SELECT i.id, i.name, i.description, i.status,
                    (SELECT COUNT(*) FROM epics e WHERE e.initiative_id = i.id) AS epic_count,
                    (SELECT COUNT(*) FROM tasks t WHERE t.initiative_id = i.id) AS task_count,
                    (SELECT COUNT(*) FROM tasks t WHERE t.initiative_id = i.id AND t.status = 'done') AS done_count,
                    CASE WHEN (SELECT COUNT(*) FROM tasks t WHERE t.initiative_id = i.id) = 0 THEN 0.0
                         ELSE ROUND(CAST((SELECT COUNT(*) FROM tasks t WHERE t.initiative_id = i.id AND t.status IN ('done', 'cancelled')) AS REAL) /
                                    CAST((SELECT COUNT(*) FROM tasks t WHERE t.initiative_id = i.id) AS REAL) * 100.0, 1)
                    END AS progress_pct,
                    (SELECT COUNT(DISTINCT td.blocked_task_id) FROM task_dependencies td
                     JOIN tasks bt ON bt.id = td.blocking_task_id
                     JOIN tasks blocked ON blocked.id = td.blocked_task_id
                     WHERE blocked.initiative_id = i.id
                       AND blocked.status NOT IN ('done', 'cancelled')
                       AND bt.status NOT IN ('done', 'cancelled')) AS blocked_count
             FROM initiatives i
             WHERE i.workspace_id = $1
             ORDER BY i.name",
        )
        .bind(ws_str)
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|row| {
                let id_str: String = row.try_get("id")?;
                Ok(InitiativeWithCounts {
                    id: parse_uuid(&id_str)?,
                    name: row.try_get("name")?,
                    description: row.try_get("description")?,
                    status: row.try_get("status")?,
                    epic_count: row.try_get("epic_count")?,
                    task_count: row.try_get("task_count")?,
                    done_count: row.try_get("done_count")?,
                    progress_pct: row.try_get("progress_pct")?,
                    blocked_count: row.try_get("blocked_count")?,
                })
            })
            .collect()
    }

    async fn fetch_recent_decisions(&self, ws_str: &str) -> Result<Vec<Decision>> {
        let rows = sqlx::query(
            "SELECT d.id, d.workspace_id, COALESCE(w.name, '') AS workspace_name,
                    d.initiative_id, COALESCE(i.name, '') AS initiative_name,
                    d.epic_id, COALESCE(ep.name, '') AS epic_name,
                    d.title, d.content, d.status, d.metadata, d.created_at, d.updated_at
             FROM decisions d
             LEFT JOIN workspaces w ON w.id = d.workspace_id
             LEFT JOIN initiatives i ON i.id = d.initiative_id
             LEFT JOIN epics ep ON ep.id = d.epic_id
             WHERE d.workspace_id = $1 ORDER BY d.created_at DESC LIMIT 10",
        )
        .bind(ws_str)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(super::decision::row_to_decision).collect()
    }

    async fn fetch_status_counts(&self, ws_str: &str) -> Result<Vec<StatusCount>> {
        let rows = sqlx::query(
            "SELECT status, COUNT(*) AS count
             FROM tasks WHERE workspace_id = $1
             GROUP BY status ORDER BY status",
        )
        .bind(ws_str)
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|row| {
                let status_str: String = row.try_get("status")?;
                let status: TaskStatus =
                    serde_json::from_value(serde_json::Value::String(status_str)).map_err(|e| {
                        crate::error::Error::Internal(format!("invalid task status: {e}"))
                    })?;
                Ok(StatusCount {
                    status,
                    count: row.try_get("count")?,
                })
            })
            .collect()
    }

    async fn fetch_priority_counts(&self, ws_str: &str) -> Result<Vec<PriorityCount>> {
        let rows = sqlx::query(
            "SELECT priority, COUNT(*) AS count
             FROM tasks WHERE workspace_id = $1
             GROUP BY priority ORDER BY priority",
        )
        .bind(ws_str)
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|row| {
                let priority_str: String = row.try_get("priority")?;
                let priority: TaskPriority =
                    serde_json::from_value(serde_json::Value::String(priority_str)).map_err(
                        |e| crate::error::Error::Internal(format!("invalid task priority: {e}")),
                    )?;
                Ok(PriorityCount {
                    priority,
                    count: row.try_get("count")?,
                })
            })
            .collect()
    }

    async fn fetch_blocked_count(&self, ws_str: &str) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(DISTINCT td.blocked_task_id) AS cnt
             FROM task_dependencies td
             JOIN tasks bt ON bt.id = td.blocking_task_id
             JOIN tasks blocked ON blocked.id = td.blocked_task_id
             WHERE blocked.workspace_id = $1
               AND blocked.status NOT IN ('done', 'cancelled')
               AND bt.status NOT IN ('done', 'cancelled')",
        )
        .bind(ws_str)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("cnt")?)
    }

    pub(crate) async fn get_next_tasks_impl(
        &self,
        workspace_id: Uuid,
        limit: i64,
    ) -> Result<Vec<TaskSummary>> {
        self.scan_task_summaries(
            "SELECT t.id, t.name, t.status, t.priority, t.assignee,
                    ep.name AS epic_name, i.name AS initiative_name, t.domain_tags, t.created_at
             FROM tasks t
             JOIN epics ep ON ep.id = t.epic_id
             JOIN initiatives i ON i.id = t.initiative_id
             WHERE t.workspace_id = $1
               AND t.assignee = ''
               AND t.status NOT IN ('done', 'cancelled')
               AND NOT EXISTS (
                   SELECT 1 FROM task_dependencies td
                   JOIN tasks bt ON bt.id = td.blocking_task_id
                   WHERE td.blocked_task_id = t.id
                     AND bt.status NOT IN ('done', 'cancelled')
               )
             ORDER BY
               CASE t.priority
                 WHEN 'critical' THEN 1
                 WHEN 'high' THEN 2
                 WHEN 'medium' THEN 3
                 WHEN 'low' THEN 4
               END,
               t.created_at ASC
             LIMIT $2",
            vec![workspace_id.to_string(), limit.to_string()],
        )
        .await
    }
}
