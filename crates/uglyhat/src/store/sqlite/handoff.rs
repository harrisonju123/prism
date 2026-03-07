use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;
use crate::store::HandoffFilters;

impl SqliteStore {
    pub(crate) async fn create_handoff_impl(
        &self,
        task_id: Uuid,
        agent_name: &str,
        summary: &str,
        findings: Vec<String>,
        blockers: Vec<String>,
        next_steps: Vec<String>,
        artifacts: Option<serde_json::Value>,
    ) -> Result<Handoff> {
        // Look up workspace_id from task
        let ws_row = sqlx::query("SELECT workspace_id FROM tasks WHERE id = $1")
            .bind(task_id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound(format!("task {task_id} not found")))?;
        let ws_id_str: String = ws_row.try_get("workspace_id")?;

        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO handoffs (id, task_id, workspace_id, agent_name, summary, findings, blockers, next_steps, artifacts, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             RETURNING id, task_id, workspace_id, agent_name, summary, findings, blockers, next_steps, artifacts, created_at",
        )
        .bind(id.to_string())
        .bind(task_id.to_string())
        .bind(&ws_id_str)
        .bind(agent_name)
        .bind(summary)
        .bind(json_array_to_str(&findings))
        .bind(json_array_to_str(&blockers))
        .bind(json_array_to_str(&next_steps))
        .bind(opt_value_to_str(&artifacts))
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        row_to_handoff(&row)
    }

    pub(crate) async fn get_handoffs_by_task_impl(&self, task_id: Uuid) -> Result<Vec<Handoff>> {
        let rows = sqlx::query(
            "SELECT id, task_id, workspace_id, agent_name, summary, findings, blockers, next_steps, artifacts, created_at
             FROM handoffs WHERE task_id = $1 ORDER BY created_at DESC",
        )
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_handoff).collect()
    }

    pub(crate) async fn list_handoffs_by_workspace_impl(
        &self,
        workspace_id: Uuid,
        filters: HandoffFilters,
    ) -> Result<Vec<Handoff>> {
        let mut clauses = vec!["workspace_id = $1".to_string()];
        let mut args: Vec<String> = vec![workspace_id.to_string()];

        if let Some(ref since) = filters.since {
            args.push(since.to_rfc3339());
            clauses.push(format!("created_at >= ${}", args.len()));
        }
        if let Some(ref agent) = filters.agent {
            args.push(agent.clone());
            clauses.push(format!("agent_name = ${}", args.len()));
        }

        let query = format!(
            "SELECT id, task_id, workspace_id, agent_name, summary, findings, blockers, next_steps, artifacts, created_at
             FROM handoffs WHERE {} ORDER BY created_at DESC LIMIT 50",
            clauses.join(" AND "),
        );

        let mut q = sqlx::query(&query);
        for arg in &args {
            q = q.bind(arg);
        }

        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_handoff).collect()
    }

    pub(crate) async fn list_handoffs_by_epic_impl(&self, epic_id: Uuid) -> Result<Vec<Handoff>> {
        let rows = sqlx::query(
            "SELECT h.id, h.task_id, h.workspace_id, h.agent_name, h.summary, h.findings, h.blockers, h.next_steps, h.artifacts, h.created_at
             FROM handoffs h
             JOIN tasks t ON t.id = h.task_id
             WHERE t.epic_id = $1
             ORDER BY h.created_at DESC",
        )
        .bind(epic_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_handoff).collect()
    }
}

fn row_to_handoff(row: &sqlx::sqlite::SqliteRow) -> Result<Handoff> {
    let id_str: String = row.try_get("id")?;
    let task_id_str: String = row.try_get("task_id")?;
    let ws_id_str: String = row.try_get("workspace_id")?;
    let findings_str: String = row.try_get("findings")?;
    let blockers_str: String = row.try_get("blockers")?;
    let next_steps_str: String = row.try_get("next_steps")?;
    let artifacts_str: Option<String> = row.try_get("artifacts")?;
    let created_str: String = row.try_get("created_at")?;

    Ok(Handoff {
        id: parse_uuid(&id_str)?,
        task_id: parse_uuid(&task_id_str)?,
        workspace_id: parse_uuid(&ws_id_str)?,
        agent_name: row.try_get("agent_name")?,
        summary: row.try_get("summary")?,
        findings: parse_json_array(&findings_str),
        blockers: parse_json_array(&blockers_str),
        next_steps: parse_json_array(&next_steps_str),
        artifacts: str_to_opt_value(artifacts_str),
        created_at: parse_time(&created_str)?,
    })
}
