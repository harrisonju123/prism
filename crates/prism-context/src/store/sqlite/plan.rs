use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn create_plan_impl(&self, workspace_id: Uuid, intent: &str) -> Result<Plan> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO plans (id, workspace_id, intent, status, created_at, updated_at)
             VALUES ($1,$2,$3,'draft',$4,$5)
             RETURNING id, workspace_id, intent, status, created_at, updated_at",
        )
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
        let row = sqlx::query(
            "SELECT id, workspace_id, intent, status, created_at, updated_at
             FROM plans WHERE workspace_id = $1 AND id = $2",
        )
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
        let row = sqlx::query(
            "UPDATE plans SET status = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING id, workspace_id, intent, status, created_at, updated_at",
        )
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
                sqlx::query(
                    "SELECT id, workspace_id, intent, status, created_at, updated_at
                 FROM plans WHERE workspace_id = $1 AND status = $2
                 ORDER BY created_at DESC",
                )
                .bind(workspace_id.to_string())
                .bind(s.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT id, workspace_id, intent, status, created_at, updated_at
                 FROM plans WHERE workspace_id = $1
                 ORDER BY created_at DESC",
                )
                .bind(workspace_id.to_string())
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(row_to_plan).collect()
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
        created_at: time "created_at",
        updated_at: time "updated_at",
    }
}
