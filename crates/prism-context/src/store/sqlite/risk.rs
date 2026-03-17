use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn create_risk_impl(
        &self,
        workspace_id: Uuid,
        thread_id: Option<Uuid>,
        title: &str,
        description: &str,
        category: &str,
        severity: RiskSeverity,
        source_agent: Option<&str>,
        tags: Vec<String>,
    ) -> Result<Risk> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO risks
                 (id, workspace_id, thread_id, title, description, category,
                  severity, status, source_agent, tags, created_at, updated_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,'identified',$8,$9,$10,$11)
             RETURNING id, workspace_id, thread_id, title, description, category,
                       severity, status, mitigation_plan, verification_criteria,
                       source_agent, tags, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(thread_id.map(|u| u.to_string()))
        .bind(title)
        .bind(description)
        .bind(category)
        .bind(severity.to_string())
        .bind(source_agent)
        .bind(json_array_to_str(&tags))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        let risk = row_to_risk(&row)?;

        self.log_activity_fire_and_forget(
            workspace_id,
            source_agent.unwrap_or(""),
            "risk_created",
            "risk",
            risk.id,
            &format!("[{severity}] {title}"),
            thread_id,
        )
        .await;

        Ok(risk)
    }

    pub(crate) async fn update_risk_status_impl(
        &self,
        workspace_id: Uuid,
        risk_id: Uuid,
        status: RiskStatus,
        mitigation_plan: Option<&str>,
        verification_criteria: Option<&str>,
    ) -> Result<Risk> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE risks
             SET status = $1, mitigation_plan = COALESCE($2, mitigation_plan),
                 verification_criteria = COALESCE($3, verification_criteria),
                 updated_at = $4
             WHERE workspace_id = $5 AND id = $6
             RETURNING id, workspace_id, thread_id, title, description, category,
                       severity, status, mitigation_plan, verification_criteria,
                       source_agent, tags, created_at, updated_at",
        )
        .bind(status.to_string())
        .bind(mitigation_plan)
        .bind(verification_criteria)
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(risk_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("risk {risk_id} not found")))?;
        row_to_risk(&row)
    }

    pub(crate) async fn get_risk_impl(
        &self,
        workspace_id: Uuid,
        risk_id: Uuid,
    ) -> Result<Risk> {
        let row = sqlx::query(
            "SELECT id, workspace_id, thread_id, title, description, category,
                    severity, status, mitigation_plan, verification_criteria,
                    source_agent, tags, created_at, updated_at
             FROM risks WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id.to_string())
        .bind(risk_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("risk {risk_id} not found")))?;
        row_to_risk(&row)
    }

    pub(crate) async fn list_risks_impl(
        &self,
        workspace_id: Uuid,
        status: Option<RiskStatus>,
        thread_id: Option<Uuid>,
    ) -> Result<Vec<Risk>> {
        let rows = match (status, thread_id) {
            (Some(s), Some(tid)) => {
                sqlx::query(
                    "SELECT id, workspace_id, thread_id, title, description, category,
                            severity, status, mitigation_plan, verification_criteria,
                            source_agent, tags, created_at, updated_at
                     FROM risks WHERE workspace_id = $1 AND status = $2 AND thread_id = $3
                     ORDER BY created_at DESC",
                )
                .bind(workspace_id.to_string())
                .bind(s.to_string())
                .bind(tid.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            (Some(s), None) => {
                sqlx::query(
                    "SELECT id, workspace_id, thread_id, title, description, category,
                            severity, status, mitigation_plan, verification_criteria,
                            source_agent, tags, created_at, updated_at
                     FROM risks WHERE workspace_id = $1 AND status = $2
                     ORDER BY created_at DESC",
                )
                .bind(workspace_id.to_string())
                .bind(s.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            (None, Some(tid)) => {
                sqlx::query(
                    "SELECT id, workspace_id, thread_id, title, description, category,
                            severity, status, mitigation_plan, verification_criteria,
                            source_agent, tags, created_at, updated_at
                     FROM risks WHERE workspace_id = $1 AND thread_id = $2
                     ORDER BY created_at DESC",
                )
                .bind(workspace_id.to_string())
                .bind(tid.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            (None, None) => {
                sqlx::query(
                    "SELECT id, workspace_id, thread_id, title, description, category,
                            severity, status, mitigation_plan, verification_criteria,
                            source_agent, tags, created_at, updated_at
                     FROM risks WHERE workspace_id = $1
                     ORDER BY created_at DESC",
                )
                .bind(workspace_id.to_string())
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(row_to_risk).collect()
    }

    pub(crate) async fn list_unverified_risks_impl(
        &self,
        workspace_id: Uuid,
        agent_name: Option<&str>,
    ) -> Result<Vec<Risk>> {
        let rows = if let Some(agent) = agent_name {
            sqlx::query(
                "SELECT id, workspace_id, thread_id, title, description, category,
                        severity, status, mitigation_plan, verification_criteria,
                        source_agent, tags, created_at, updated_at
                 FROM risks
                 WHERE workspace_id = $1
                   AND status NOT IN ('verified','accepted')
                   AND source_agent = $2
                 ORDER BY created_at DESC",
            )
            .bind(workspace_id.to_string())
            .bind(agent)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, workspace_id, thread_id, title, description, category,
                        severity, status, mitigation_plan, verification_criteria,
                        source_agent, tags, created_at, updated_at
                 FROM risks
                 WHERE workspace_id = $1
                   AND status NOT IN ('verified','accepted')
                 ORDER BY created_at DESC",
            )
            .bind(workspace_id.to_string())
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter().map(row_to_risk).collect()
    }
}

row_to_struct! {
    pub(super) fn row_to_risk(row) -> Risk {
        id: uuid "id",
        workspace_id: uuid "workspace_id",
        thread_id: opt_uuid "thread_id",
        title: str "title",
        description: custom "description" => {
            row.try_get::<String, _>("description").unwrap_or_default()
        },
        category: custom "category" => {
            row.try_get::<String, _>("category").unwrap_or_default()
        },
        severity: custom "severity" => {
            let s: String = row.try_get::<String, _>("severity")?;
            RiskSeverity::from_str(&s)
                .ok_or_else(|| Error::Internal(format!("invalid risk severity: {s}")))?
        },
        status: custom "status" => {
            let s: String = row.try_get::<String, _>("status")?;
            RiskStatus::from_str(&s)
                .ok_or_else(|| Error::Internal(format!("invalid risk status: {s}")))?
        },
        mitigation_plan: custom "mitigation_plan" => {
            row.try_get::<Option<String>, _>("mitigation_plan").unwrap_or(None)
        },
        verification_criteria: custom "verification_criteria" => {
            row.try_get::<Option<String>, _>("verification_criteria").unwrap_or(None)
        },
        source_agent: custom "source_agent" => {
            row.try_get::<Option<String>, _>("source_agent").unwrap_or(None)
        },
        tags: json_array "tags",
        created_at: time "created_at",
        updated_at: time "updated_at",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> SqliteStore {
        SqliteStore::open_memory().await.expect("open memory store")
    }

    async fn workspace(store: &SqliteStore) -> Uuid {
        store.init_workspace_impl("test", "").await.expect("init workspace").id
    }

    #[tokio::test]
    async fn risk_crud() {
        let s = store().await;
        let ws = workspace(&s).await;

        let risk = s
            .create_risk_impl(
                ws, None, "Auth token leak", "JWT stored in localStorage",
                "security", RiskSeverity::High, Some("claude"), vec!["auth".to_string()],
            )
            .await
            .expect("create risk");
        assert_eq!(risk.status, RiskStatus::Identified);
        assert_eq!(risk.severity, RiskSeverity::High);

        let updated = s
            .update_risk_status_impl(ws, risk.id, RiskStatus::Mitigated, Some("Use httpOnly cookies"), None)
            .await
            .expect("update status");
        assert_eq!(updated.status, RiskStatus::Mitigated);
        assert_eq!(updated.mitigation_plan.as_deref(), Some("Use httpOnly cookies"));

        let fetched = s.get_risk_impl(ws, risk.id).await.expect("get risk");
        assert_eq!(fetched.id, risk.id);

        let all = s.list_risks_impl(ws, None, None).await.expect("list risks");
        assert_eq!(all.len(), 1);

        let unverified = s.list_unverified_risks_impl(ws, None).await.expect("unverified");
        assert_eq!(unverified.len(), 1);

        // Mark verified — should no longer appear in unverified list
        s.update_risk_status_impl(ws, risk.id, RiskStatus::Verified, None, Some("Test passes"))
            .await
            .expect("verify");
        let unverified2 = s.list_unverified_risks_impl(ws, None).await.expect("unverified2");
        assert!(unverified2.is_empty());
    }
}
