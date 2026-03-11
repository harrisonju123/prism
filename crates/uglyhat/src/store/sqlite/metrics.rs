use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use crate::error::Result;
use crate::model::AgentMetrics;

impl SqliteStore {
    pub(super) async fn agent_metrics_impl(&self, workspace_id: Uuid) -> Result<Vec<AgentMetrics>> {
        let ws_str = workspace_id.to_string();

        // Single query: conditional aggregation + CTE for avg completion time.
        // Binds ws_str twice (once in the CTE, once in the main filter).
        let rows = sqlx::query(r#"
            WITH task_durations AS (
                SELECT actor, entity_id,
                    (julianday(MAX(created_at)) - julianday(MIN(created_at))) * 24 * 60 AS duration_mins
                FROM activity_log
                WHERE workspace_id = ? AND entity_type = 'task'
                GROUP BY actor, entity_id
                HAVING COUNT(*) > 1
            )
            SELECT
                a.actor,
                SUM(CASE WHEN a.action = 'claim' THEN 1 ELSE 0 END) AS tasks_claimed,
                SUM(CASE WHEN a.action IN ('update', 'complete') AND a.summary LIKE '%done%' THEN 1 ELSE 0 END) AS tasks_done,
                SUM(CASE WHEN a.action = 'block' THEN 1 ELSE 0 END) AS tasks_blocked,
                COALESCE(AVG(d.duration_mins), 0.0) AS avg_completion_mins
            FROM activity_log a
            LEFT JOIN task_durations d ON a.actor = d.actor
            WHERE a.workspace_id = ? AND a.actor != ''
            GROUP BY a.actor
            ORDER BY a.actor
        "#)
        .bind(&ws_str)
        .bind(&ws_str)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::Error::Internal(format!("agent_metrics: {e}")))?;

        rows.iter()
            .map(|row| {
                Ok(AgentMetrics {
                    agent: row.try_get("actor")?,
                    tasks_claimed: row.try_get("tasks_claimed")?,
                    tasks_done: row.try_get("tasks_done")?,
                    tasks_blocked: row.try_get("tasks_blocked")?,
                    avg_completion_mins: row.try_get("avg_completion_mins")?,
                })
            })
            .collect()
    }
}
