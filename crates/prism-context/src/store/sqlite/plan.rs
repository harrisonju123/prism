use chrono::Utc;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

/// Full column list for SELECT queries on plans.
const PLAN_COLS: &str = "id, workspace_id, intent, status, description, constraints,
     current_phase, assumptions, blockers, files_touched, autonomy_level,
     created_at, updated_at";

impl SqliteStore {
    pub(crate) async fn create_plan_impl(&self, workspace_id: Uuid, intent: &str) -> Result<Plan> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(&format!(
            "INSERT INTO plans (id, workspace_id, intent, status, description, constraints,
              current_phase, assumptions, blockers, files_touched, autonomy_level,
              created_at, updated_at)
             VALUES ($1,$2,$3,'draft','','[]','investigate','[]','[]','[]','supervised',$4,$5)
             RETURNING {PLAN_COLS}",
        ))
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(intent)
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        let plan = row_to_plan(&row)?;

        self.log_activity_fire_and_forget(
            workspace_id,
            "",
            "created",
            "plan",
            plan.id,
            &format!("Created plan: {}", &intent[..intent.len().min(60)]),
            None,
        )
        .await;

        Ok(plan)
    }

    pub(crate) async fn get_plan_impl(&self, workspace_id: Uuid, plan_id: Uuid) -> Result<Plan> {
        let row = sqlx::query(&format!(
            "SELECT {PLAN_COLS} FROM plans WHERE workspace_id = $1 AND id = $2",
        ))
        .bind(workspace_id.to_string())
        .bind(plan_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("plan {plan_id} not found")))?;
        row_to_plan(&row)
    }

    pub(crate) async fn update_plan_status_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        status: PlanStatus,
    ) -> Result<Plan> {
        let now = now_rfc3339();
        let row = sqlx::query(&format!(
            "UPDATE plans SET status = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING {PLAN_COLS}",
        ))
        .bind(status.to_string())
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(plan_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("plan {plan_id} not found")))?;
        let plan = row_to_plan(&row)?;

        self.log_activity_fire_and_forget(
            workspace_id,
            "system",
            "plan_status_changed",
            "plan",
            plan_id,
            &format!("Plan status changed to {status}"),
            None,
        )
        .await;

        Ok(plan)
    }

    pub(crate) async fn list_plans_impl(
        &self,
        workspace_id: Uuid,
        status: Option<PlanStatus>,
    ) -> Result<Vec<Plan>> {
        let rows = match status {
            Some(s) => {
                sqlx::query(&format!(
                    "SELECT {PLAN_COLS} FROM plans WHERE workspace_id = $1 AND status = $2
                     ORDER BY created_at DESC",
                ))
                .bind(workspace_id.to_string())
                .bind(s.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(&format!(
                    "SELECT {PLAN_COLS} FROM plans WHERE workspace_id = $1
                     ORDER BY created_at DESC",
                ))
                .bind(workspace_id.to_string())
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(row_to_plan).collect()
    }

    pub(crate) async fn update_plan_phase_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        phase: MissionPhase,
    ) -> Result<Plan> {
        let now = now_rfc3339();
        let row = sqlx::query(&format!(
            "UPDATE plans SET current_phase = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING {PLAN_COLS}",
        ))
        .bind(phase.to_string())
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(plan_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("plan {plan_id} not found")))?;
        row_to_plan(&row)
    }

    pub(crate) async fn update_plan_metadata_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        description: Option<&str>,
        constraints: Option<Vec<String>>,
        autonomy: Option<AutonomyLevel>,
    ) -> Result<Plan> {
        let now = now_rfc3339();
        let mut plan = self.get_plan_impl(workspace_id, plan_id).await?;
        if let Some(d) = description {
            plan.description = d.to_string();
        }
        if let Some(c) = constraints {
            plan.constraints = c;
        }
        if let Some(a) = autonomy {
            plan.autonomy_level = a;
        }
        let row = sqlx::query(&format!(
            "UPDATE plans SET description = $1, constraints = $2, autonomy_level = $3, updated_at = $4
             WHERE workspace_id = $5 AND id = $6
             RETURNING {PLAN_COLS}",
        ))
        .bind(&plan.description)
        .bind(serde_json::to_string(&plan.constraints).unwrap_or_else(|_| "[]".to_string()))
        .bind(plan.autonomy_level.to_string())
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(plan_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("plan {plan_id} not found")))?;
        row_to_plan(&row)
    }

    pub(crate) async fn add_plan_assumption_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        text: &str,
        source_agent: &str,
    ) -> Result<Plan> {
        let now = now_rfc3339();
        let mut plan = self.get_plan_impl(workspace_id, plan_id).await?;
        plan.assumptions.push(Assumption::new(text, source_agent));
        let assumptions_json = serde_json::to_string(&plan.assumptions)
            .unwrap_or_else(|_| "[]".to_string());
        let row = sqlx::query(&format!(
            "UPDATE plans SET assumptions = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING {PLAN_COLS}",
        ))
        .bind(&assumptions_json)
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(plan_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("plan {plan_id} not found")))?;
        row_to_plan(&row)
    }

    pub(crate) async fn update_plan_assumption_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        index: usize,
        status: AssumptionStatus,
    ) -> Result<Plan> {
        let now = now_rfc3339();
        let mut plan = self.get_plan_impl(workspace_id, plan_id).await?;
        if index >= plan.assumptions.len() {
            return Err(Error::NotFound(format!("assumption index {index} out of range")));
        }
        plan.assumptions[index].status = status;
        let assumptions_json = serde_json::to_string(&plan.assumptions)
            .unwrap_or_else(|_| "[]".to_string());
        let row = sqlx::query(&format!(
            "UPDATE plans SET assumptions = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING {PLAN_COLS}",
        ))
        .bind(&assumptions_json)
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(plan_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("plan {plan_id} not found")))?;
        row_to_plan(&row)
    }

    pub(crate) async fn add_plan_blocker_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        text: &str,
        source_agent: &str,
    ) -> Result<Plan> {
        let now = now_rfc3339();
        let mut plan = self.get_plan_impl(workspace_id, plan_id).await?;
        plan.blockers.push(Blocker::new(text, source_agent));
        let blockers_json = serde_json::to_string(&plan.blockers)
            .unwrap_or_else(|_| "[]".to_string());
        let row = sqlx::query(&format!(
            "UPDATE plans SET blockers = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING {PLAN_COLS}",
        ))
        .bind(&blockers_json)
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(plan_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("plan {plan_id} not found")))?;
        row_to_plan(&row)
    }

    pub(crate) async fn resolve_plan_blocker_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        index: usize,
    ) -> Result<Plan> {
        let now = now_rfc3339();
        let mut plan = self.get_plan_impl(workspace_id, plan_id).await?;
        if index >= plan.blockers.len() {
            return Err(Error::NotFound(format!("blocker index {index} out of range")));
        }
        plan.blockers[index].status = BlockerStatus::Resolved;
        plan.blockers[index].resolved_at = Some(Utc::now());
        let blockers_json = serde_json::to_string(&plan.blockers)
            .unwrap_or_else(|_| "[]".to_string());
        let row = sqlx::query(&format!(
            "UPDATE plans SET blockers = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING {PLAN_COLS}",
        ))
        .bind(&blockers_json)
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(plan_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("plan {plan_id} not found")))?;
        row_to_plan(&row)
    }

    pub(crate) async fn record_plan_file_touched_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        path: &str,
    ) -> Result<Plan> {
        let now = now_rfc3339();
        let mut plan = self.get_plan_impl(workspace_id, plan_id).await?;
        if !plan.files_touched.contains(&path.to_string()) {
            plan.files_touched.push(path.to_string());
        }
        let files_json = serde_json::to_string(&plan.files_touched)
            .unwrap_or_else(|_| "[]".to_string());
        let row = sqlx::query(&format!(
            "UPDATE plans SET files_touched = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING {PLAN_COLS}",
        ))
        .bind(&files_json)
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(plan_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("plan {plan_id} not found")))?;
        row_to_plan(&row)
    }

    pub(crate) async fn get_active_plan_impl(
        &self,
        workspace_id: Uuid,
    ) -> Result<Option<Plan>> {
        let row = sqlx::query(&format!(
            "SELECT {PLAN_COLS} FROM plans
             WHERE workspace_id = $1 AND status IN ('active','approved')
             ORDER BY updated_at DESC
             LIMIT 1",
        ))
        .bind(workspace_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(row_to_plan(&r)?)),
            None => Ok(None),
        }
    }
}

row_to_struct! {
    pub(super) fn row_to_plan(row) -> Plan {
        id: uuid "id",
        workspace_id: uuid "workspace_id",
        intent: str "intent",
        status: custom "status" => {
            let s: String = row.try_get::<String, _>("status")?;
            PlanStatus::from_str(&s)
                .ok_or_else(|| Error::Internal(format!("invalid plan status: {s}")))?
        },
        description: custom "description" => {
            row.try_get::<String, _>("description").unwrap_or_default()
        },
        constraints: custom "constraints" => {
            let raw = row.try_get::<String, _>("constraints").unwrap_or_else(|_| "[]".to_string());
            serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default()
        },
        current_phase: custom "current_phase" => {
            let s = row.try_get::<String, _>("current_phase").unwrap_or_else(|_| "investigate".to_string());
            MissionPhase::from_str(&s).unwrap_or_default()
        },
        assumptions: custom "assumptions" => {
            let raw = row.try_get::<String, _>("assumptions").unwrap_or_else(|_| "[]".to_string());
            serde_json::from_str::<Vec<Assumption>>(&raw).unwrap_or_default()
        },
        blockers: custom "blockers" => {
            let raw = row.try_get::<String, _>("blockers").unwrap_or_else(|_| "[]".to_string());
            serde_json::from_str::<Vec<Blocker>>(&raw).unwrap_or_default()
        },
        files_touched: custom "files_touched" => {
            let raw = row.try_get::<String, _>("files_touched").unwrap_or_else(|_| "[]".to_string());
            serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default()
        },
        autonomy_level: custom "autonomy_level" => {
            let s = row.try_get::<String, _>("autonomy_level").unwrap_or_else(|_| "supervised".to_string());
            AutonomyLevel::from_str(&s).unwrap_or_default()
        },
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
    async fn plan_mission_metadata() {
        let s = store().await;
        let ws = workspace(&s).await;

        let plan = s.create_plan_impl(ws, "Add OAuth callback").await.expect("create plan");
        assert_eq!(plan.current_phase, MissionPhase::Investigate);
        assert!(plan.assumptions.is_empty());

        // Advance phase
        let p2 = s.update_plan_phase_impl(ws, plan.id, MissionPhase::Implement).await.expect("phase");
        assert_eq!(p2.current_phase, MissionPhase::Implement);

        // Add assumption
        let p3 = s.add_plan_assumption_impl(ws, plan.id, "OAuth endpoint is public", "claude").await.expect("assumption");
        assert_eq!(p3.assumptions.len(), 1);
        assert_eq!(p3.assumptions[0].status, AssumptionStatus::Unverified);

        // Confirm assumption
        let p4 = s.update_plan_assumption_impl(ws, plan.id, 0, AssumptionStatus::Confirmed).await.expect("confirm");
        assert_eq!(p4.assumptions[0].status, AssumptionStatus::Confirmed);

        // Add blocker
        let p5 = s.add_plan_blocker_impl(ws, plan.id, "Redirect URL not confirmed", "claude").await.expect("blocker");
        assert_eq!(p5.blockers.len(), 1);

        // Resolve blocker
        let p6 = s.resolve_plan_blocker_impl(ws, plan.id, 0).await.expect("resolve");
        assert_eq!(p6.blockers[0].status, BlockerStatus::Resolved);

        // Record file touched
        let p7 = s.record_plan_file_touched_impl(ws, plan.id, "src/oauth.rs").await.expect("file");
        assert!(p7.files_touched.contains(&"src/oauth.rs".to_string()));

        // Get active plan (need to make it active first)
        s.update_plan_status_impl(ws, plan.id, PlanStatus::Active).await.expect("activate");
        let active = s.get_active_plan_impl(ws).await.expect("get active");
        assert!(active.is_some());
    }
}
