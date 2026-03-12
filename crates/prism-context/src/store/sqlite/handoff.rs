use sqlx::Row as _;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

pub(super) fn row_to_handoff(row: &sqlx::sqlite::SqliteRow) -> Result<Handoff> {
    let constraints_str: String = row.try_get("constraints")?;
    let constraints: HandoffConstraints =
        serde_json::from_str(&constraints_str).unwrap_or_default();
    let mode_str: String = row.try_get("mode")?;
    let status_str: String = row.try_get("status")?;
    let result_str: Option<String> = row.try_get("result")?;
    Ok(Handoff {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        workspace_id: parse_uuid(&row.try_get::<String, _>("workspace_id")?)?,
        from_agent_id: parse_uuid(&row.try_get::<String, _>("from_agent_id")?)?,
        to_agent_id: parse_opt_uuid(row.try_get("to_agent_id")?)?,
        thread_id: parse_opt_uuid(row.try_get("thread_id")?)?,
        task: row.try_get("task")?,
        constraints,
        mode: HandoffMode::from_str(&mode_str).unwrap_or(HandoffMode::DelegateAndAwait),
        status: HandoffStatus::from_str(&status_str).unwrap_or(HandoffStatus::Pending),
        result: result_str.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: parse_time(&row.try_get::<String, _>("created_at")?)?,
        updated_at: parse_time(&row.try_get::<String, _>("updated_at")?)?,
    })
}

impl SqliteStore {
    pub(crate) async fn create_handoff_impl(
        &self,
        workspace_id: Uuid,
        from_agent: &str,
        task: &str,
        thread_id: Option<Uuid>,
        constraints: HandoffConstraints,
        mode: HandoffMode,
    ) -> Result<Handoff> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();

        // Look up the from_agent's ID
        let from_agent_id: String =
            sqlx::query_scalar("SELECT id FROM agents WHERE workspace_id = $1 AND name = $2")
                .bind(workspace_id.to_string())
                .bind(from_agent)
                .fetch_optional(&self.pool)
                .await?
                .ok_or_else(|| Error::NotFound(format!("agent {from_agent:?} not found")))?;

        let constraints_json =
            serde_json::to_string(&constraints).unwrap_or_else(|_| "{}".to_string());

        let row = sqlx::query(
            "INSERT INTO handoffs (id, workspace_id, from_agent_id, to_agent_id, thread_id, task, constraints, mode, status, created_at, updated_at)
             VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, 'pending', $8, $9)
             RETURNING id, workspace_id, from_agent_id, to_agent_id, thread_id, task, constraints, mode, status, result, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(&from_agent_id)
        .bind(thread_id.map(|u| u.to_string()))
        .bind(task)
        .bind(&constraints_json)
        .bind(mode.to_string())
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        let handoff = row_to_handoff(&row)?;

        self.log_activity_fire_and_forget(
            workspace_id,
            from_agent,
            "handoff_created",
            "handoff",
            handoff.id,
            &format!("Handoff: {task}"),
            None,
        )
        .await;

        Ok(handoff)
    }

    pub(crate) async fn accept_handoff_impl(
        &self,
        workspace_id: Uuid,
        handoff_id: Uuid,
        agent_name: &str,
    ) -> Result<Handoff> {
        let now = now_rfc3339();

        // Look up agent ID
        let agent_id: String =
            sqlx::query_scalar("SELECT id FROM agents WHERE workspace_id = $1 AND name = $2")
                .bind(workspace_id.to_string())
                .bind(agent_name)
                .fetch_optional(&self.pool)
                .await?
                .ok_or_else(|| Error::NotFound(format!("agent {agent_name:?} not found")))?;

        let row = sqlx::query(
            "UPDATE handoffs SET to_agent_id = $1, status = 'accepted', updated_at = $2
             WHERE id = $3 AND workspace_id = $4 AND status = 'pending'
             RETURNING id, workspace_id, from_agent_id, to_agent_id, thread_id, task, constraints, mode, status, result, created_at, updated_at",
        )
        .bind(&agent_id)
        .bind(&now)
        .bind(handoff_id.to_string())
        .bind(workspace_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("pending handoff {handoff_id} not found")))?;

        row_to_handoff(&row)
    }

    pub(crate) async fn complete_handoff_impl(
        &self,
        workspace_id: Uuid,
        handoff_id: Uuid,
        result: serde_json::Value,
    ) -> Result<Handoff> {
        let now = now_rfc3339();
        let result_json = serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string());

        let row = sqlx::query(
            "UPDATE handoffs SET status = 'completed', result = $1, updated_at = $2
             WHERE id = $3 AND workspace_id = $4 AND status IN ('accepted', 'running', 'pending')
             RETURNING id, workspace_id, from_agent_id, to_agent_id, thread_id, task, constraints, mode, status, result, created_at, updated_at",
        )
        .bind(&result_json)
        .bind(&now)
        .bind(handoff_id.to_string())
        .bind(workspace_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| {
            Error::NotFound(format!("handoff {handoff_id} not found or already completed"))
        })?;

        row_to_handoff(&row)
    }

    pub(crate) async fn list_handoffs_impl(
        &self,
        workspace_id: Uuid,
        agent_name: Option<&str>,
        status: Option<HandoffStatus>,
    ) -> Result<Vec<Handoff>> {
        let mut clauses = vec!["h.workspace_id = $1".to_string()];
        let mut args: Vec<String> = vec![workspace_id.to_string()];

        if let Some(name) = agent_name {
            args.push(name.to_string());
            let idx = args.len();
            clauses.push(format!("(fa.name = ${idx} OR ta.name = ${idx})"));
        }

        if let Some(ref s) = status {
            args.push(s.to_string());
            clauses.push(format!("h.status = ${}", args.len()));
        }

        let query = format!(
            "SELECT h.id, h.workspace_id, h.from_agent_id, h.to_agent_id, h.thread_id,
                    h.task, h.constraints, h.mode, h.status, h.result, h.created_at, h.updated_at
             FROM handoffs h
             LEFT JOIN agents fa ON fa.id = h.from_agent_id
             LEFT JOIN agents ta ON ta.id = h.to_agent_id
             WHERE {}
             ORDER BY h.created_at DESC
             LIMIT 100",
            clauses.join(" AND "),
        );

        let mut q = sqlx::query(&query);
        for arg in &args {
            q = q.bind(arg);
        }

        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_handoff).collect()
    }
}
