use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::task::row_to_task_summary;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn checkin_agent_impl(
        &self,
        workspace_id: Uuid,
        name: &str,
        capabilities: Vec<String>,
    ) -> Result<CheckinResponse> {
        let mut tx = self.pool.begin().await?;

        // Capture previous last_checkin before upsert
        let prev_checkin: Option<String> =
            sqlx::query("SELECT last_checkin FROM agents WHERE workspace_id = $1 AND name = $2")
                .bind(workspace_id.to_string())
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?
                .and_then(|row| row.try_get("last_checkin").ok());

        let now = now_rfc3339();
        let caps = json_array_to_str(&capabilities);

        // Upsert agent
        let agent_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO agents (id, workspace_id, name, capabilities, last_checkin, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (workspace_id, name) DO UPDATE
             SET capabilities = excluded.capabilities, last_checkin = excluded.last_checkin, updated_at = excluded.updated_at",
        )
        .bind(agent_id.to_string())
        .bind(workspace_id.to_string())
        .bind(name)
        .bind(&caps)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        // Read back agent
        let a_row = sqlx::query(
            "SELECT id, workspace_id, name, capabilities, last_checkin, created_at, updated_at
             FROM agents WHERE workspace_id = $1 AND name = $2",
        )
        .bind(workspace_id.to_string())
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
        let agent = row_to_agent(&a_row)?;

        // Create session
        let session_id = Uuid::new_v4();
        let s_row = sqlx::query(
            "INSERT INTO agent_sessions (id, agent_id, workspace_id, started_at, ended_at, summary, created_at)
             VALUES ($1, $2, $3, $4, NULL, '', $5)
             RETURNING id, agent_id, workspace_id, started_at, ended_at, summary, created_at",
        )
        .bind(session_id.to_string())
        .bind(agent.id.to_string())
        .bind(workspace_id.to_string())
        .bind(&now)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await?;
        let session = row_to_agent_session(&s_row)?;

        // Get assigned tasks
        let task_rows = sqlx::query(
            "SELECT t.id, t.name, t.status, t.priority, t.assignee,
                    ep.name AS epic_name, i.name AS initiative_name, t.domain_tags, t.created_at
             FROM tasks t
             JOIN epics ep ON ep.id = t.epic_id
             JOIN initiatives i ON i.id = t.initiative_id
             WHERE t.workspace_id = $1
               AND t.assignee = $2
               AND t.status NOT IN ('done', 'cancelled')
             ORDER BY
               CASE t.priority
                 WHEN 'critical' THEN 1
                 WHEN 'high' THEN 2
                 WHEN 'medium' THEN 3
                 WHEN 'low' THEN 4
               END,
               t.created_at ASC",
        )
        .bind(workspace_id.to_string())
        .bind(name)
        .fetch_all(&mut *tx)
        .await?;
        let assigned_tasks: Vec<TaskSummary> = task_rows
            .iter()
            .map(row_to_task_summary)
            .collect::<Result<_>>()?;

        // Fetch activity since last checkin
        let mut recent_activity = Vec::new();
        if let Some(ref prev) = prev_checkin {
            let act_rows = sqlx::query(
                "SELECT id, workspace_id, actor, action, entity_type, entity_id, summary, detail, created_at
                 FROM activity_log
                 WHERE workspace_id = $1 AND created_at >= $2
                 ORDER BY created_at DESC
                 LIMIT 20",
            )
            .bind(workspace_id.to_string())
            .bind(prev)
            .fetch_all(&mut *tx)
            .await?;
            for row in &act_rows {
                recent_activity.push(super::activity::row_to_activity_entry(row)?);
            }
        }

        tx.commit().await?;

        Ok(CheckinResponse {
            agent,
            session,
            assigned_tasks,
            recent_activity,
        })
    }

    pub(crate) async fn checkout_agent_impl(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        summary: &str,
    ) -> Result<AgentSession> {
        let now = now_rfc3339();

        // Find open session
        let session_row = sqlx::query(
            "SELECT s.id FROM agent_sessions s
             JOIN agents a ON a.id = s.agent_id
             WHERE a.workspace_id = $1 AND a.name = $2 AND s.ended_at IS NULL
             ORDER BY s.started_at DESC
             LIMIT 1",
        )
        .bind(workspace_id.to_string())
        .bind(agent_name)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("no open session for agent {agent_name:?}")))?;
        let session_id: String = session_row.try_get("id")?;

        // Update it
        let row = sqlx::query(
            "UPDATE agent_sessions SET ended_at = $1, summary = $2 WHERE id = $3
             RETURNING id, agent_id, workspace_id, started_at, ended_at, summary, created_at",
        )
        .bind(&now)
        .bind(summary)
        .bind(&session_id)
        .fetch_one(&self.pool)
        .await?;

        // Clear current_task_id on checkout
        sqlx::query(
            "UPDATE agents SET current_task_id = NULL, updated_at = $1
             WHERE workspace_id = $2 AND name = $3",
        )
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(agent_name)
        .execute(&self.pool)
        .await?;

        row_to_agent_session(&row)
    }

    pub(crate) async fn list_agent_statuses_impl(&self, workspace_id: Uuid) -> Result<Vec<AgentStatus>> {
        let rows = sqlx::query(
            "SELECT a.name, a.current_task_id, a.last_checkin,
                    COALESCE(t.name, '') AS current_task_name,
                    EXISTS(SELECT 1 FROM agent_sessions s WHERE s.agent_id = a.id AND s.ended_at IS NULL) AS session_open
             FROM agents a
             LEFT JOIN tasks t ON t.id = a.current_task_id
             WHERE a.workspace_id = $1
             ORDER BY a.name",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(|row| {
            let task_id_str: Option<String> = row.try_get("current_task_id")?;
            let task_name: String = row.try_get("current_task_name")?;
            let checkin_str: Option<String> = row.try_get("last_checkin")?;
            let session_open: bool = row.try_get("session_open")?;
            Ok(AgentStatus {
                name: row.try_get("name")?,
                session_open,
                current_task_id: task_id_str.as_deref().and_then(|s| s.parse::<Uuid>().ok()),
                current_task_name: if task_name.is_empty() { None } else { Some(task_name) },
                last_checkin: parse_opt_time(checkin_str)?,
            })
        }).collect()
    }

    pub(crate) async fn list_agents_impl(&self, workspace_id: Uuid) -> Result<Vec<Agent>> {
        let rows = sqlx::query(
            "SELECT id, workspace_id, name, capabilities, last_checkin, created_at, updated_at
             FROM agents WHERE workspace_id = $1 ORDER BY name",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_agent).collect()
    }
}

fn row_to_agent(row: &sqlx::sqlite::SqliteRow) -> Result<Agent> {
    let id_str: String = row.try_get("id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let caps_str: String = row.try_get("capabilities")?;
    let checkin_str: Option<String> = row.try_get("last_checkin")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Agent {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        name: row.try_get("name")?,
        capabilities: parse_json_array(&caps_str),
        last_checkin: parse_opt_time(checkin_str)?,
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}

fn row_to_agent_session(row: &sqlx::sqlite::SqliteRow) -> Result<AgentSession> {
    let id_str: String = row.try_get("id")?;
    let agent_id_str: String = row.try_get("agent_id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let started_str: String = row.try_get("started_at")?;
    let ended_str: Option<String> = row.try_get("ended_at")?;
    let created_str: String = row.try_get("created_at")?;

    Ok(AgentSession {
        id: parse_uuid(&id_str)?,
        agent_id: parse_uuid(&agent_id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        started_at: parse_time(&started_str)?,
        ended_at: parse_opt_time(ended_str)?,
        summary: row.try_get("summary")?,
        created_at: parse_time(&created_str)?,
    })
}
