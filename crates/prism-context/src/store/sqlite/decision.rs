use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn save_decision_impl(
        &self,
        workspace_id: Uuid,
        title: &str,
        content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
        scope: DecisionScope,
    ) -> Result<Decision> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO decisions (id, workspace_id, thread_id, title, content, status, scope, tags, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             RETURNING id, workspace_id, thread_id, title, content, status, scope, superseded_by, supersedes, tags, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(thread_id.map(|u| u.to_string()))
        .bind(title)
        .bind(content)
        .bind(DecisionStatus::Active.to_string())
        .bind(scope.to_string())
        .bind(json_array_to_str(&tags))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        let decision = row_to_decision(&row)?;

        // Phase 4: auto-create notification rows for workspace-scoped decisions
        if decision.scope == DecisionScope::Workspace {
            self.create_decision_notifications(workspace_id, decision.id, None)
                .await;
        }

        self.log_activity_fire_and_forget(
            workspace_id,
            "",
            "decided",
            "decision",
            decision.id,
            &format!("Decision: {title}"),
            None,
        )
        .await;

        Ok(decision)
    }

    pub(crate) async fn supersede_decision_impl(
        &self,
        workspace_id: Uuid,
        old_id: Uuid,
        new_title: &str,
        new_content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
    ) -> Result<Decision> {
        let now = now_rfc3339();
        let new_id = Uuid::new_v4();

        // Verify old decision exists and is active, and get its scope
        let old_scope: String = sqlx::query_scalar(
            "SELECT scope FROM decisions WHERE id = $1 AND workspace_id = $2 AND status = 'active'",
        )
        .bind(old_id.to_string())
        .bind(workspace_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("active decision {old_id} not found")))?;

        // Insert new decision that supersedes the old one
        let row = sqlx::query(
            "INSERT INTO decisions (id, workspace_id, thread_id, title, content, status, scope, supersedes, tags, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, 'active', $6, $7, $8, $9, $10)
             RETURNING id, workspace_id, thread_id, title, content, status, scope, superseded_by, supersedes, tags, created_at, updated_at",
        )
        .bind(new_id.to_string())
        .bind(workspace_id.to_string())
        .bind(thread_id.map(|u| u.to_string()))
        .bind(new_title)
        .bind(new_content)
        .bind(&old_scope)
        .bind(old_id.to_string())
        .bind(json_array_to_str(&tags))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        // Mark old decision as superseded
        sqlx::query(
            "UPDATE decisions SET status = 'superseded', superseded_by = $1, updated_at = $2
             WHERE id = $3",
        )
        .bind(new_id.to_string())
        .bind(&now)
        .bind(old_id.to_string())
        .execute(&self.pool)
        .await?;

        let decision = row_to_decision(&row)?;

        // Notify other agents if workspace-scoped
        if decision.scope == DecisionScope::Workspace {
            self.create_decision_notifications(workspace_id, decision.id, None)
                .await;
        }

        self.log_activity_fire_and_forget(
            workspace_id,
            "",
            "superseded",
            "decision",
            decision.id,
            &format!("Superseded decision {old_id} with: {new_title}"),
            None,
        )
        .await;

        Ok(decision)
    }

    pub(crate) async fn revoke_decision_impl(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<Decision> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE decisions SET status = 'revoked', updated_at = $1
             WHERE id = $2 AND workspace_id = $3 AND status = 'active'
             RETURNING id, workspace_id, thread_id, title, content, status, scope, superseded_by, supersedes, tags, created_at, updated_at",
        )
        .bind(&now)
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("active decision {id} not found")))?;

        let decision = row_to_decision(&row)?;

        self.log_activity_fire_and_forget(
            workspace_id,
            "",
            "revoked",
            "decision",
            decision.id,
            &format!("Revoked decision: {}", decision.title),
            None,
        )
        .await;

        Ok(decision)
    }

    pub(crate) async fn pending_decision_notifications_impl(
        &self,
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
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_decision).collect()
    }

    pub(crate) async fn acknowledge_decisions_impl(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        decision_ids: Vec<Uuid>,
    ) -> Result<()> {
        if decision_ids.is_empty() {
            return Ok(());
        }

        // Find agent ID
        let agent_id: String =
            sqlx::query_scalar("SELECT id FROM agents WHERE workspace_id = $1 AND name = $2")
                .bind(workspace_id.to_string())
                .bind(agent_name)
                .fetch_optional(&self.pool)
                .await?
                .ok_or_else(|| Error::NotFound(format!("agent {agent_name:?} not found")))?;

        // Batch update: build IN clause for all decision IDs
        let placeholders: Vec<String> = decision_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 2))
            .collect();
        let query = format!(
            "UPDATE decision_notifications SET acknowledged = 1
             WHERE agent_id = $1 AND decision_id IN ({})",
            placeholders.join(", ")
        );
        let mut q = sqlx::query(&query).bind(&agent_id);
        for did in &decision_ids {
            q = q.bind(did.to_string());
        }
        q.execute(&self.pool).await?;

        Ok(())
    }

    pub(crate) async fn list_decisions_impl(
        &self,
        workspace_id: Uuid,
        thread_id: Option<Uuid>,
        tags: Option<Vec<String>>,
    ) -> Result<Vec<Decision>> {
        let mut clauses = vec!["workspace_id = $1".to_string()];
        let mut args: Vec<String> = vec![workspace_id.to_string()];

        if let Some(tid) = thread_id {
            args.push(tid.to_string());
            clauses.push(format!("thread_id = ${}", args.len()));
        }

        if let Some(ref tag_list) = tags {
            push_tag_clauses(tag_list, &mut clauses, &mut args);
        }

        let query = format!(
            "SELECT id, workspace_id, thread_id, title, content, status, scope, superseded_by, supersedes, tags, created_at, updated_at
             FROM decisions
             WHERE {}
             ORDER BY created_at DESC
             LIMIT 200",
            clauses.join(" AND "),
        );

        let mut q = sqlx::query(&query);
        for arg in &args {
            q = q.bind(arg);
        }

        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_decision).collect()
    }

    /// Create notification rows for all active agents, excluding the author if specified.
    async fn create_decision_notifications(
        &self,
        workspace_id: Uuid,
        decision_id: Uuid,
        exclude_agent_name: Option<&str>,
    ) {
        let now = now_rfc3339();
        let decision_id_str = decision_id.to_string();

        // Single INSERT...SELECT to batch-create notifications for all active agents
        let result = if let Some(exclude_name) = exclude_agent_name {
            sqlx::query(
                "INSERT OR IGNORE INTO decision_notifications (id, decision_id, agent_id, notified_at, acknowledged)
                 SELECT lower(hex(randomblob(16))), $1, a.id, $2, 0
                 FROM agents a
                 JOIN agent_sessions s ON s.agent_id = a.id
                 WHERE a.workspace_id = $3 AND s.ended_at IS NULL AND a.name != $4
                 GROUP BY a.id",
            )
            .bind(&decision_id_str)
            .bind(&now)
            .bind(workspace_id.to_string())
            .bind(exclude_name)
            .execute(&self.pool)
            .await
        } else {
            sqlx::query(
                "INSERT OR IGNORE INTO decision_notifications (id, decision_id, agent_id, notified_at, acknowledged)
                 SELECT lower(hex(randomblob(16))), $1, a.id, $2, 0
                 FROM agents a
                 JOIN agent_sessions s ON s.agent_id = a.id
                 WHERE a.workspace_id = $3 AND s.ended_at IS NULL
                 GROUP BY a.id",
            )
            .bind(&decision_id_str)
            .bind(&now)
            .bind(workspace_id.to_string())
            .execute(&self.pool)
            .await
        };
        let _ = result;
    }
}

row_to_struct! {
    pub(super) fn row_to_decision(row) -> Decision {
        id: uuid "id",
        workspace_id: uuid "workspace_id",
        thread_id: opt_uuid "thread_id",
        title: str "title",
        content: str "content",
        status: custom "status" => {
            let s: String = row.try_get::<String, _>("status")?;
            match s.as_str() {
                "active" => DecisionStatus::Active,
                "superseded" => DecisionStatus::Superseded,
                "revoked" => DecisionStatus::Revoked,
                other => return Err(crate::error::Error::Internal(format!("invalid decision status: {other}")))
            }
        },
        scope: custom "scope" => {
            let s: String = row.try_get::<String, _>("scope")?;
            DecisionScope::from_str(&s).unwrap_or(DecisionScope::Thread)
        },
        superseded_by: opt_uuid "superseded_by",
        supersedes: opt_uuid "supersedes",
        tags: json_array "tags",
        created_at: time "created_at",
        updated_at: time "updated_at",
    }
}
