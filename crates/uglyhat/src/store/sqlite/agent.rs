use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::thread::row_to_thread;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn checkin_impl(
        &self,
        workspace_id: Uuid,
        name: &str,
        capabilities: Vec<String>,
        thread_id: Option<Uuid>,
    ) -> Result<CheckinContext> {
        let mut tx = self.pool.begin().await?;
        let now = now_rfc3339();
        let caps = json_array_to_str(&capabilities);

        // Upsert agent
        let agent_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO agents (id, workspace_id, name, capabilities, current_thread_id, last_checkin, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (workspace_id, name) DO UPDATE
             SET capabilities = excluded.capabilities,
                 current_thread_id = excluded.current_thread_id,
                 last_checkin = excluded.last_checkin,
                 updated_at = excluded.updated_at",
        )
        .bind(agent_id.to_string())
        .bind(workspace_id.to_string())
        .bind(name)
        .bind(&caps)
        .bind(thread_id.map(|u| u.to_string()))
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        // Read back agent
        let a_row = sqlx::query(
            "SELECT id, workspace_id, name, capabilities, current_thread_id, last_checkin, created_at, updated_at
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
            "INSERT INTO agent_sessions (id, agent_id, workspace_id, thread_id, started_at, ended_at, summary, findings, files_touched, next_steps, created_at)
             VALUES ($1, $2, $3, $4, $5, NULL, '', '[]', '[]', '[]', $6)
             RETURNING id, agent_id, workspace_id, thread_id, started_at, ended_at, summary, findings, files_touched, next_steps, created_at",
        )
        .bind(session_id.to_string())
        .bind(agent.id.to_string())
        .bind(workspace_id.to_string())
        .bind(thread_id.map(|u| u.to_string()))
        .bind(&now)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await?;
        let session = row_to_agent_session(&s_row)?;

        // Get active threads
        let thread_rows = sqlx::query(
            "SELECT id, workspace_id, name, description, status, tags, created_at, updated_at
             FROM threads WHERE workspace_id = $1 AND status = 'active'
             ORDER BY updated_at DESC",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let active_threads: Vec<Thread> = thread_rows
            .iter()
            .map(row_to_thread)
            .collect::<Result<_>>()?;

        // Get global memories
        let mem_rows = sqlx::query(
            "SELECT id, workspace_id, thread_id, key, value, source, tags, created_at, updated_at
             FROM memories WHERE workspace_id = $1 AND thread_id IS NULL
             ORDER BY updated_at DESC LIMIT 50",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let global_memories: Vec<Memory> = mem_rows
            .iter()
            .map(super::memory::row_to_memory)
            .collect::<Result<_>>()?;

        // Get recent sessions (last 5 across all agents)
        let sess_rows = sqlx::query(
            "SELECT id, agent_id, workspace_id, thread_id, started_at, ended_at, summary, findings, files_touched, next_steps, created_at
             FROM agent_sessions WHERE workspace_id = $1 AND ended_at IS NOT NULL
             ORDER BY ended_at DESC LIMIT 5",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let recent_sessions: Vec<AgentSession> = sess_rows
            .iter()
            .map(row_to_agent_session)
            .collect::<Result<_>>()?;

        // Get other agents' statuses
        let agent_rows = sqlx::query(
            "SELECT a.name, a.current_thread_id, a.last_checkin,
                    COALESCE(t.name, '') AS current_thread_name,
                    EXISTS(SELECT 1 FROM agent_sessions s WHERE s.agent_id = a.id AND s.ended_at IS NULL) AS session_open
             FROM agents a
             LEFT JOIN threads t ON t.id = a.current_thread_id
             WHERE a.workspace_id = $1
             ORDER BY a.name",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let other_agents: Vec<AgentStatus> = agent_rows
            .iter()
            .map(row_to_agent_status)
            .collect::<Result<_>>()?;

        tx.commit().await?;

        Ok(CheckinContext {
            agent,
            session,
            active_threads,
            global_memories,
            recent_sessions,
            other_agents,
        })
    }

    pub(crate) async fn checkout_impl(
        &self,
        workspace_id: Uuid,
        name: &str,
        summary: &str,
        findings: Vec<String>,
        files_touched: Vec<String>,
        next_steps: Vec<String>,
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
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("no open session for agent {name:?}")))?;
        let session_id: String = session_row.try_get("id")?;

        // Update session with checkout data
        let row = sqlx::query(
            "UPDATE agent_sessions
             SET ended_at = $1, summary = $2, findings = $3, files_touched = $4, next_steps = $5
             WHERE id = $6
             RETURNING id, agent_id, workspace_id, thread_id, started_at, ended_at, summary, findings, files_touched, next_steps, created_at",
        )
        .bind(&now)
        .bind(summary)
        .bind(json_array_to_str(&findings))
        .bind(json_array_to_str(&files_touched))
        .bind(json_array_to_str(&next_steps))
        .bind(&session_id)
        .fetch_one(&self.pool)
        .await?;

        // Clear current_thread_id on checkout
        sqlx::query(
            "UPDATE agents SET current_thread_id = NULL, updated_at = $1
             WHERE workspace_id = $2 AND name = $3",
        )
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(name)
        .execute(&self.pool)
        .await?;

        row_to_agent_session(&row)
    }

    pub(crate) async fn list_agents_impl(&self, workspace_id: Uuid) -> Result<Vec<AgentStatus>> {
        let rows = sqlx::query(
            "SELECT a.name, a.current_thread_id, a.last_checkin,
                    COALESCE(t.name, '') AS current_thread_name,
                    EXISTS(SELECT 1 FROM agent_sessions s WHERE s.agent_id = a.id AND s.ended_at IS NULL) AS session_open
             FROM agents a
             LEFT JOIN threads t ON t.id = a.current_thread_id
             WHERE a.workspace_id = $1
             ORDER BY a.name",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_agent_status).collect()
    }
}

fn row_to_agent(row: &sqlx::sqlite::SqliteRow) -> Result<Agent> {
    let id_str: String = row.try_get("id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let caps_str: String = row.try_get("capabilities")?;
    let thread_str: Option<String> = row.try_get("current_thread_id")?;
    let checkin_str: Option<String> = row.try_get("last_checkin")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Agent {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        name: row.try_get("name")?,
        capabilities: parse_json_array(&caps_str),
        current_thread_id: parse_opt_uuid(thread_str)?,
        last_checkin: parse_opt_time(checkin_str)?,
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}

pub(super) fn row_to_agent_session(row: &sqlx::sqlite::SqliteRow) -> Result<AgentSession> {
    let id_str: String = row.try_get("id")?;
    let agent_id_str: String = row.try_get("agent_id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let thread_str: Option<String> = row.try_get("thread_id")?;
    let started_str: String = row.try_get("started_at")?;
    let ended_str: Option<String> = row.try_get("ended_at")?;
    let findings_str: String = row.try_get("findings")?;
    let files_str: String = row.try_get("files_touched")?;
    let next_str: String = row.try_get("next_steps")?;
    let created_str: String = row.try_get("created_at")?;

    Ok(AgentSession {
        id: parse_uuid(&id_str)?,
        agent_id: parse_uuid(&agent_id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        thread_id: parse_opt_uuid(thread_str)?,
        started_at: parse_time(&started_str)?,
        ended_at: parse_opt_time(ended_str)?,
        summary: row.try_get("summary")?,
        findings: parse_json_array(&findings_str),
        files_touched: parse_json_array(&files_str),
        next_steps: parse_json_array(&next_str),
        created_at: parse_time(&created_str)?,
    })
}

fn row_to_agent_status(row: &sqlx::sqlite::SqliteRow) -> Result<AgentStatus> {
    let thread_name: String = row.try_get("current_thread_name")?;
    let checkin_str: Option<String> = row.try_get("last_checkin")?;
    let session_open: bool = row.try_get("session_open")?;
    Ok(AgentStatus {
        name: row.try_get("name")?,
        session_open,
        current_thread: if thread_name.is_empty() {
            None
        } else {
            Some(thread_name)
        },
        last_checkin: parse_opt_time(checkin_str)?,
    })
}
