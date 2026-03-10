use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use crate::error::{Error, Result};
use crate::model::AgentMetrics;

impl SqliteStore {
    pub(crate) async fn agent_metrics_impl(&self, workspace_id: Uuid) -> Result<Vec<AgentMetrics>> {
        let rows = sqlx::query(
            "SELECT
                a.name as agent_name,
                (SELECT COUNT(*) FROM agent_sessions s WHERE s.agent_id = a.id) as total_sessions,
                (SELECT COUNT(*) FROM tasks t WHERE t.assignee = a.name AND t.workspace_id = $1 AND t.status = 'done') as total_tasks_completed,
                (SELECT COUNT(*) FROM handoffs h WHERE h.agent_name = a.name AND h.workspace_id = $1) as total_handoffs
             FROM agents a
             WHERE a.workspace_id = $1
             ORDER BY a.name",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;

        let mut metrics = Vec::new();
        for row in &rows {
            metrics.push(AgentMetrics {
                agent_name: row
                    .try_get("agent_name")
                    .map_err(|e| Error::Internal(e.to_string()))?,
                total_sessions: row.try_get("total_sessions").unwrap_or(0),
                total_tasks_completed: row.try_get("total_tasks_completed").unwrap_or(0),
                total_handoffs: row.try_get("total_handoffs").unwrap_or(0),
            });
        }

        Ok(metrics)
    }
}
