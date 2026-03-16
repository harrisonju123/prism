use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::handoff::row_to_handoff;
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
        branch: Option<String>,
        worktree_path: Option<String>,
    ) -> Result<CheckinContext> {
        let mut tx = self.pool.begin().await?;
        let now = now_rfc3339();
        let caps = json_array_to_str(&capabilities);
        let branch_val = branch.unwrap_or_default();
        let worktree_val = worktree_path.unwrap_or_default();

        // Upsert agent (set state=idle on checkin)
        let agent_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO agents (id, workspace_id, name, state, capabilities, current_thread_id, last_checkin, last_heartbeat, branch, worktree_path, created_at, updated_at)
             VALUES ($1, $2, $3, 'idle', $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (workspace_id, name) DO UPDATE
             SET capabilities = excluded.capabilities,
                 current_thread_id = excluded.current_thread_id,
                 last_checkin = excluded.last_checkin,
                 last_heartbeat = excluded.last_heartbeat,
                 branch = excluded.branch,
                 worktree_path = excluded.worktree_path,
                 state = 'idle',
                 updated_at = excluded.updated_at",
        )
        .bind(agent_id.to_string())
        .bind(workspace_id.to_string())
        .bind(name)
        .bind(&caps)
        .bind(thread_id.map(|u| u.to_string()))
        .bind(&now)
        .bind(&now)
        .bind(&branch_val)
        .bind(&worktree_val)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        // Close any open sessions from a previous run (agent re-checkin without checkout)
        sqlx::query(
            "UPDATE agent_sessions SET ended_at = $1,
               summary = CASE WHEN summary = '' THEN 'auto-closed: agent re-checkin'
                         ELSE summary || ' [auto-closed: agent re-checkin]' END
             WHERE agent_id = (SELECT id FROM agents WHERE workspace_id = $2 AND name = $3)
               AND ended_at IS NULL",
        )
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(name)
        .execute(&mut *tx)
        .await?;

        // Read back agent
        let a_row = sqlx::query(
            "SELECT id, workspace_id, name, state, capabilities, current_thread_id, last_checkin, last_heartbeat, parent_agent_id, branch, worktree_path, created_at, updated_at
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
            "INSERT INTO agent_sessions (id, agent_id, workspace_id, thread_id, started_at, ended_at, summary, findings, files_touched, next_steps, branch, worktree_path, created_at)
             VALUES ($1, $2, $3, $4, $5, NULL, '', '[]', '[]', '[]', $6, $7, $8)
             RETURNING id, agent_id, workspace_id, thread_id, started_at, ended_at, summary, findings, files_touched, next_steps, branch, worktree_path, created_at",
        )
        .bind(session_id.to_string())
        .bind(agent.id.to_string())
        .bind(workspace_id.to_string())
        .bind(thread_id.map(|u| u.to_string()))
        .bind(&now)
        .bind(&branch_val)
        .bind(&worktree_val)
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
            "SELECT id, agent_id, workspace_id, thread_id, started_at, ended_at, summary, findings, files_touched, next_steps, branch, worktree_path, created_at
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
            "SELECT a.name, a.state, a.current_thread_id, a.last_checkin, a.last_heartbeat,
                    a.branch, a.worktree_path,
                    COALESCE(t.name, '') AS current_thread_name,
                    COALESCE(p.name, '') AS parent_agent_name,
                    EXISTS(SELECT 1 FROM agent_sessions s WHERE s.agent_id = a.id AND s.ended_at IS NULL) AS session_open
             FROM agents a
             LEFT JOIN threads t ON t.id = a.current_thread_id
             LEFT JOIN agents p ON p.id = a.parent_agent_id
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

        // Pending decision notifications
        let pending_decisions = self
            .pending_decision_notifications_tx(&mut tx, workspace_id, name)
            .await?;

        // Pending handoffs (assigned to this agent or unassigned)
        let handoff_rows = sqlx::query(
            "SELECT h.id, h.workspace_id, h.from_agent_id, h.to_agent_id, h.thread_id,
                    h.task, h.constraints, h.mode, h.status, h.result, h.created_at, h.updated_at
             FROM handoffs h
             WHERE h.workspace_id = $1 AND h.status = 'pending'
               AND (h.to_agent_id = $2 OR h.to_agent_id IS NULL)
             ORDER BY h.created_at ASC",
        )
        .bind(workspace_id.to_string())
        .bind(agent.id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let pending_handoffs: Vec<Handoff> = handoff_rows
            .iter()
            .map(row_to_handoff)
            .collect::<Result<_>>()?;

        tx.commit().await?;

        self.log_activity_fire_and_forget(
            workspace_id,
            name,
            "checkin",
            "agent",
            agent.id,
            &format!("Agent '{name}' checked in"),
            agent.current_thread_id,
        )
        .await;

        Ok(CheckinContext {
            agent,
            session,
            active_threads,
            global_memories,
            recent_sessions,
            other_agents,
            pending_decisions,
            pending_handoffs,
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
        let mut tx = self.pool.begin().await?;

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
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| Error::NotFound(format!("no open session for agent {name:?}")))?;
        let session_id: String = session_row.try_get("id")?;

        // Update session with checkout data
        let row = sqlx::query(
            "UPDATE agent_sessions
             SET ended_at = $1, summary = $2, findings = $3, files_touched = $4, next_steps = $5
             WHERE id = $6
             RETURNING id, agent_id, workspace_id, thread_id, started_at, ended_at, summary, findings, files_touched, next_steps, branch, worktree_path, created_at",
        )
        .bind(&now)
        .bind(summary)
        .bind(json_array_to_str(&findings))
        .bind(json_array_to_str(&files_touched))
        .bind(json_array_to_str(&next_steps))
        .bind(&session_id)
        .fetch_one(&mut *tx)
        .await?;

        let session = row_to_agent_session(&row)?;

        // Set state=idle, clear current_thread_id on checkout
        sqlx::query(
            "UPDATE agents SET current_thread_id = NULL, state = 'idle', updated_at = $1
             WHERE workspace_id = $2 AND name = $3",
        )
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(name)
        .execute(&mut *tx)
        .await?;

        // Insert Completed inbox entry (atomic with checkout)
        let body = serde_json::json!({
            "task_name": name,
            "description": "",
            "branch": if session.branch.is_empty() { name } else { &session.branch },
            "diff_preview": "",
            "session_cost_usd": serde_json::Value::Null,
            "test_summary": serde_json::Value::Null,
            "files_touched": &files_touched,
            "summary": summary,
            "findings": &findings,
        })
        .to_string();
        let (ref_type, ref_id) = match session.thread_id {
            Some(tid) => (Some("thread"), Some(tid)),
            None => (None, None),
        };
        super::inbox::insert_inbox_entry_tx(
            &mut tx,
            workspace_id,
            InboxEntryType::Completed,
            name,
            &body,
            InboxSeverity::Info,
            Some(name),
            ref_type,
            ref_id,
        )
        .await?;

        tx.commit().await?;

        // Release any file claims held by this agent on checkout
        let _ = self.release_all_claims_for_agent_impl(workspace_id, name).await;

        self.log_activity_fire_and_forget(
            workspace_id,
            name,
            "checkout",
            "agent_session",
            session.id,
            &format!("Agent '{name}' checked out"),
            session.thread_id,
        )
        .await;

        Ok(session)
    }

    pub(crate) async fn list_agents_impl(&self, workspace_id: Uuid) -> Result<Vec<AgentStatus>> {
        let rows = sqlx::query(
            "SELECT a.name, a.state, a.current_thread_id, a.last_checkin, a.last_heartbeat,
                    a.branch, a.worktree_path,
                    COALESCE(t.name, '') AS current_thread_name,
                    COALESCE(p.name, '') AS parent_agent_name,
                    EXISTS(SELECT 1 FROM agent_sessions s WHERE s.agent_id = a.id AND s.ended_at IS NULL) AS session_open
             FROM agents a
             LEFT JOIN threads t ON t.id = a.current_thread_id
             LEFT JOIN agents p ON p.id = a.parent_agent_id
             WHERE a.workspace_id = $1
             ORDER BY a.name",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_agent_status).collect()
    }

    pub(crate) async fn heartbeat_impl(&self, workspace_id: Uuid, name: &str) -> Result<()> {
        let now = now_rfc3339();
        let result = sqlx::query(
            "UPDATE agents SET last_heartbeat = $1, updated_at = $2
             WHERE workspace_id = $3 AND name = $4",
        )
        .bind(&now)
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(name)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("agent {name:?} not found")));
        }
        Ok(())
    }

    pub(crate) async fn set_agent_state_impl(
        &self,
        workspace_id: Uuid,
        name: &str,
        state: AgentState,
    ) -> Result<()> {
        let now = now_rfc3339();
        let id_str: Option<String> = sqlx::query_scalar(
            "UPDATE agents SET state = $1, updated_at = $2
             WHERE workspace_id = $3 AND name = $4
             RETURNING id",
        )
        .bind(state.to_string())
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        let Some(id_str) = id_str else {
            return Err(Error::NotFound(format!("agent {name:?} not found")));
        };

        if let Ok(agent_id) = parse_uuid(&id_str) {
            self.log_activity_fire_and_forget(
                workspace_id,
                name,
                "state_changed",
                "agent",
                agent_id,
                &format!("Agent '{name}' state changed to {state}"),
                None,
            )
            .await;
        }

        Ok(())
    }

    pub(crate) async fn reap_dead_agents_impl(
        &self,
        workspace_id: Uuid,
        timeout_secs: i64,
    ) -> Result<Vec<String>> {
        let now = now_rfc3339();
        let threshold = (chrono::Utc::now() - chrono::Duration::seconds(timeout_secs)).to_rfc3339();

        let mut tx = self.pool.begin().await?;

        // Atomically mark stale agents as dead and return their ids and names.
        let rows = sqlx::query(
            "UPDATE agents SET state = 'dead', updated_at = $1
             WHERE workspace_id = $2
               AND state != 'dead'
               AND last_heartbeat IS NOT NULL
               AND last_heartbeat < $3
             RETURNING id, name",
        )
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(&threshold)
        .fetch_all(&mut *tx)
        .await?;

        let reaped: Vec<(String, String)> = rows
            .iter()
            .map(|r| Ok((r.try_get::<String, _>("id")?, r.try_get::<String, _>("name")?)))
            .collect::<std::result::Result<_, sqlx::Error>>()?;

        let names: Vec<String> = reaped.iter().map(|(_, n)| n.clone()).collect();

        if !names.is_empty() {
            // Close open sessions for reaped agents
            sqlx::query(
                "UPDATE agent_sessions SET ended_at = $1,
                   summary = CASE WHEN summary = '' THEN 'auto-closed: agent reaped'
                             ELSE summary || ' [auto-closed: agent reaped]' END
                 WHERE agent_id IN (SELECT id FROM agents WHERE workspace_id = $2 AND state = 'dead')
                   AND ended_at IS NULL",
            )
            .bind(&now)
            .bind(workspace_id.to_string())
            .execute(&mut *tx)
            .await?;

            // Insert Blocked inbox entry for each reaped agent
            for agent_name in &names {
                let title = format!("Agent '{agent_name}' reaped (heartbeat timeout)");
                super::inbox::insert_inbox_entry_tx(
                    &mut tx,
                    workspace_id,
                    InboxEntryType::Blocked,
                    &title,
                    "Agent heartbeat missed; marked dead and session auto-closed.",
                    InboxSeverity::Warning,
                    Some("system"),
                    None,
                    None,
                )
                .await?;
            }
        }

        tx.commit().await?;

        // Release file claims for all reaped agents
        for (_, agent_name) in &reaped {
            let _ = self.release_all_claims_for_agent_impl(workspace_id, agent_name).await;
        }

        for (id_str, agent_name) in &reaped {
            if let Ok(agent_id) = parse_uuid(id_str) {
                self.log_activity_fire_and_forget(
                    workspace_id,
                    "system",
                    "reaped",
                    "agent",
                    agent_id,
                    &format!("Agent '{agent_name}' reaped"),
                    None,
                )
                .await;
            }
        }

        Ok(names)
    }

    pub(crate) async fn update_session_impl(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        summary: &str,
        files_touched: Vec<String>,
    ) -> Result<()> {
        let id_str: Option<String> = sqlx::query_scalar(
            "UPDATE agent_sessions SET summary = $1, files_touched = $2
             WHERE agent_id = (SELECT id FROM agents WHERE workspace_id = $3 AND name = $4)
               AND ended_at IS NULL
             RETURNING id",
        )
        .bind(summary)
        .bind(json_array_to_str(&files_touched))
        .bind(workspace_id.to_string())
        .bind(agent_name)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(id_str) = id_str {
            if let Ok(session_id) = parse_uuid(&id_str) {
                self.log_activity_fire_and_forget(
                    workspace_id,
                    agent_name,
                    "session_updated",
                    "agent_session",
                    session_id,
                    &format!("Session updated for agent '{agent_name}'"),
                    None,
                )
                .await;
            }
        }

        Ok(())
    }

    /// Helper: fetch pending decision notifications inside a transaction.
    async fn pending_decision_notifications_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        workspace_id: Uuid,
        agent_name: &str,
    ) -> Result<Vec<Decision>> {
        let rows = sqlx::query(
            "SELECT d.id, d.workspace_id, d.thread_id, d.title, d.content, d.status, d.scope,
                    d.superseded_by, d.supersedes, d.tags, d.created_at, d.updated_at
             FROM decision_notifications dn
             JOIN decisions d ON d.id = dn.decision_id
             JOIN agents a ON a.id = dn.agent_id
             WHERE a.workspace_id = $1 AND a.name = $2 AND dn.acknowledged = 0
             ORDER BY d.created_at ASC",
        )
        .bind(workspace_id.to_string())
        .bind(agent_name)
        .fetch_all(&mut **tx)
        .await?;

        rows.iter().map(super::decision::row_to_decision).collect()
    }
}

row_to_struct! {
    fn row_to_agent(row) -> Agent {
        id: uuid "id",
        workspace_id: uuid "workspace_id",
        name: str "name",
        state: custom "state" => {
            let s: String = row.try_get::<String, _>("state")?;
            AgentState::from_str(&s).unwrap_or(AgentState::Idle)
        },
        capabilities: json_array "capabilities",
        current_thread_id: opt_uuid "current_thread_id",
        last_checkin: opt_time "last_checkin",
        last_heartbeat: opt_time "last_heartbeat",
        parent_agent_id: opt_uuid "parent_agent_id",
        branch: str "branch",
        worktree_path: str "worktree_path",
        created_at: time "created_at",
        updated_at: time "updated_at",
    }
}

row_to_struct! {
    pub(super) fn row_to_agent_session(row) -> AgentSession {
        id: uuid "id",
        agent_id: uuid "agent_id",
        workspace_id: uuid "workspace_id",
        thread_id: opt_uuid "thread_id",
        started_at: time "started_at",
        ended_at: opt_time "ended_at",
        summary: str "summary",
        findings: json_array "findings",
        files_touched: json_array "files_touched",
        next_steps: json_array "next_steps",
        branch: str "branch",
        worktree_path: str "worktree_path",
        created_at: time "created_at",
    }
}

row_to_struct! {
    fn row_to_agent_status(row) -> AgentStatus {
        name: str "name",
        state: custom "state" => {
            let s: String = row.try_get::<String, _>("state")?;
            AgentState::from_str(&s).unwrap_or(AgentState::Idle)
        },
        session_open: bool "session_open",
        current_thread: custom "current_thread_name" => {
            let s: String = row.try_get::<String, _>("current_thread_name")?;
            if s.is_empty() { None } else { Some(s) }
        },
        last_checkin: opt_time "last_checkin",
        last_heartbeat: opt_time "last_heartbeat",
        parent_agent: custom "parent_agent_name" => {
            let s: String = row.try_get::<String, _>("parent_agent_name")?;
            if s.is_empty() { None } else { Some(s) }
        },
        branch: custom "branch" => {
            let s: String = row.try_get::<String, _>("branch")?;
            if s.is_empty() { None } else { Some(s) }
        },
        worktree_path: custom "worktree_path" => {
            let s: String = row.try_get::<String, _>("worktree_path")?;
            if s.is_empty() { None } else { Some(s) }
        },
    }
}
