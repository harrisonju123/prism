use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn create_work_package_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Option<Uuid>,
        intent: &str,
        acceptance_criteria: Vec<String>,
        ordinal: i32,
        depends_on: Vec<Uuid>,
        tags: Vec<String>,
    ) -> Result<WorkPackage> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let deps_json = json_uuid_array_to_str(&depends_on);
        let row = sqlx::query(
            "INSERT INTO work_packages
                 (id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                  status, depends_on, tags, created_at, updated_at)
             VALUES ($1,$2,$3,$4,$5,$6,'planned',$7,$8,$9,$10)
             RETURNING id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                       status, depends_on, thread_id, assigned_agent, tags,
                       progress_note, progress_updated_at,
                       validation_status, validation_evidence, change_rationale,
                       created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(plan_id.map(|u| u.to_string()))
        .bind(intent)
        .bind(json_array_to_str(&acceptance_criteria))
        .bind(ordinal)
        .bind(&deps_json)
        .bind(json_array_to_str(&tags))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        let wp = row_to_work_package(&row)?;

        self.log_activity_fire_and_forget(
            workspace_id,
            "",
            "created",
            "work_package",
            wp.id,
            &format!("Created work package: {}", &intent[..intent.len().min(60)]),
            wp.thread_id,
        )
        .await;

        Ok(wp)
    }

    pub(crate) async fn get_work_package_impl(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
    ) -> Result<WorkPackage> {
        let row = sqlx::query(
            "SELECT id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                    status, depends_on, thread_id, assigned_agent, tags,
                    progress_note, progress_updated_at,
                    validation_status, validation_evidence, change_rationale,
                    created_at, updated_at
             FROM work_packages WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id.to_string())
        .bind(wp_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("work package {wp_id} not found")))?;
        row_to_work_package(&row)
    }

    pub(crate) async fn update_work_package_status_impl(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        status: WorkPackageStatus,
    ) -> Result<WorkPackage> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE work_packages SET status = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                       status, depends_on, thread_id, assigned_agent, tags,
                       progress_note, progress_updated_at,
                       validation_status, validation_evidence, change_rationale,
                       created_at, updated_at",
        )
        .bind(status.to_string())
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(wp_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("work package {wp_id} not found")))?;
        let wp = row_to_work_package(&row)?;

        self.log_activity_fire_and_forget(
            workspace_id,
            "system",
            "work_package_status_changed",
            "work_package",
            wp_id,
            &format!("Work package status changed to {status}"),
            wp.thread_id,
        )
        .await;

        Ok(wp)
    }

    pub(crate) async fn assign_work_package_impl(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        agent_name: &str,
        thread_id: Uuid,
    ) -> Result<WorkPackage> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE work_packages
             SET assigned_agent = $1, thread_id = $2, status = 'in_progress', updated_at = $3
             WHERE workspace_id = $4 AND id = $5
             RETURNING id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                       status, depends_on, thread_id, assigned_agent, tags,
                       progress_note, progress_updated_at,
                       validation_status, validation_evidence, change_rationale,
                       created_at, updated_at",
        )
        .bind(agent_name)
        .bind(thread_id.to_string())
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(wp_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("work package {wp_id} not found")))?;
        let wp = row_to_work_package(&row)?;

        self.log_activity_fire_and_forget(
            workspace_id,
            agent_name,
            "work_package_assigned",
            "work_package",
            wp_id,
            &format!("Assigned to '{agent_name}'"),
            Some(thread_id),
        )
        .await;

        Ok(wp)
    }

    pub(crate) async fn list_work_packages_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Option<Uuid>,
        status: Option<WorkPackageStatus>,
    ) -> Result<Vec<WorkPackage>> {
        let rows = match (plan_id, status) {
            (Some(pid), Some(s)) => {
                sqlx::query(
                    "SELECT id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                        status, depends_on, thread_id, assigned_agent, tags,
                        progress_note, progress_updated_at,
                        validation_status, validation_evidence, change_rationale,
                        created_at, updated_at
                 FROM work_packages
                 WHERE workspace_id = $1 AND plan_id = $2 AND status = $3
                 ORDER BY ordinal ASC",
                )
                .bind(workspace_id.to_string())
                .bind(pid.to_string())
                .bind(s.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            (Some(pid), None) => {
                sqlx::query(
                    "SELECT id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                        status, depends_on, thread_id, assigned_agent, tags,
                        progress_note, progress_updated_at,
                        validation_status, validation_evidence, change_rationale,
                        created_at, updated_at
                 FROM work_packages
                 WHERE workspace_id = $1 AND plan_id = $2
                 ORDER BY ordinal ASC",
                )
                .bind(workspace_id.to_string())
                .bind(pid.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            (None, Some(s)) => {
                sqlx::query(
                    "SELECT id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                        status, depends_on, thread_id, assigned_agent, tags,
                        progress_note, progress_updated_at,
                        validation_status, validation_evidence, change_rationale,
                        created_at, updated_at
                 FROM work_packages
                 WHERE workspace_id = $1 AND status = $2
                 ORDER BY ordinal ASC",
                )
                .bind(workspace_id.to_string())
                .bind(s.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            (None, None) => {
                sqlx::query(
                    "SELECT id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                        status, depends_on, thread_id, assigned_agent, tags,
                        progress_note, progress_updated_at,
                        validation_status, validation_evidence, change_rationale,
                        created_at, updated_at
                 FROM work_packages
                 WHERE workspace_id = $1
                 ORDER BY ordinal ASC",
                )
                .bind(workspace_id.to_string())
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(row_to_work_package).collect()
    }

    pub(crate) async fn update_work_package_progress_impl(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        status: WorkPackageStatus,
        progress_note: &str,
    ) -> Result<WorkPackage> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE work_packages
             SET status = $1, progress_note = $2, progress_updated_at = $3, updated_at = $3
             WHERE workspace_id = $4 AND id = $5
             RETURNING id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                       status, depends_on, thread_id, assigned_agent, tags,
                       progress_note, progress_updated_at,
                       validation_status, validation_evidence, change_rationale,
                       created_at, updated_at",
        )
        .bind(status.to_string())
        .bind(progress_note)
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(wp_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("work package {wp_id} not found")))?;
        let wp = row_to_work_package(&row)?;

        self.log_activity_fire_and_forget(
            workspace_id,
            "system",
            "work_package_progress_updated",
            "work_package",
            wp_id,
            &format!("Progress: {}", &progress_note[..progress_note.len().min(80)]),
            wp.thread_id,
        )
        .await;

        Ok(wp)
    }

    pub(crate) async fn refresh_work_package_readiness_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
    ) -> Result<Vec<WorkPackage>> {
        // Fetch all WPs for this plan to evaluate readiness in memory.
        let all = self
            .list_work_packages_impl(workspace_id, Some(plan_id), None)
            .await?;

        let done_ids: std::collections::HashSet<Uuid> = all
            .iter()
            .filter(|wp| wp.status == WorkPackageStatus::Done)
            .map(|wp| wp.id)
            .collect();

        let mut newly_ready = Vec::new();
        for wp in &all {
            if wp.status != WorkPackageStatus::Planned {
                continue;
            }
            // Ready when all dependencies are Done (or there are no deps)
            if wp.depends_on.iter().all(|dep| done_ids.contains(dep)) {
                let updated = self
                    .update_work_package_status_impl(workspace_id, wp.id, WorkPackageStatus::Ready)
                    .await?;
                newly_ready.push(updated);
            }
        }
        Ok(newly_ready)
    }

    pub(crate) async fn record_validation_evidence_impl(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        evidence: ValidationEvidence,
    ) -> Result<WorkPackage> {
        let now = now_rfc3339();
        let mut wp = self.get_work_package_impl(workspace_id, wp_id).await?;
        wp.validation_evidence.push(evidence);
        let evidence_json = serde_json::to_string(&wp.validation_evidence)
            .unwrap_or_else(|_| "[]".to_string());
        let row = sqlx::query(
            "UPDATE work_packages SET validation_evidence = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                       status, depends_on, thread_id, assigned_agent, tags,
                       progress_note, progress_updated_at,
                       validation_status, validation_evidence, change_rationale,
                       created_at, updated_at",
        )
        .bind(&evidence_json)
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(wp_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("work_package {wp_id} not found")))?;
        row_to_work_package(&row)
    }

    pub(crate) async fn update_validation_status_impl(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        status: ValidationStatus,
    ) -> Result<WorkPackage> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE work_packages SET validation_status = $1, updated_at = $2
             WHERE workspace_id = $3 AND id = $4
             RETURNING id, workspace_id, plan_id, intent, acceptance_criteria, ordinal,
                       status, depends_on, thread_id, assigned_agent, tags,
                       progress_note, progress_updated_at,
                       validation_status, validation_evidence, change_rationale,
                       created_at, updated_at",
        )
        .bind(status.to_string())
        .bind(&now)
        .bind(workspace_id.to_string())
        .bind(wp_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("work_package {wp_id} not found")))?;
        row_to_work_package(&row)
    }
}

row_to_struct! {
    pub(super) fn row_to_work_package(row) -> WorkPackage {
        id: uuid "id",
        workspace_id: uuid "workspace_id",
        plan_id: opt_uuid "plan_id",
        intent: str "intent",
        acceptance_criteria: json_array "acceptance_criteria",
        ordinal: custom "ordinal" => {
            row.try_get::<i32, _>("ordinal")?
        },
        status: custom "status" => {
            let s: String = row.try_get::<String, _>("status")?;
            WorkPackageStatus::from_str(&s)
                .ok_or_else(|| Error::Internal(format!("invalid wp status: {s}")))?
        },
        depends_on: custom "depends_on" => {
            let raw: String = row.try_get::<String, _>("depends_on").unwrap_or_default();
            let strings: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
            strings.iter().filter_map(|s| s.parse::<uuid::Uuid>().ok()).collect::<Vec<_>>()
        },
        thread_id: opt_uuid "thread_id",
        assigned_agent: custom "assigned_agent" => {
            row.try_get::<Option<String>, _>("assigned_agent")?
        },
        tags: json_array "tags",
        progress_note: custom "progress_note" => {
            row.try_get::<Option<String>, _>("progress_note").unwrap_or(None)
        },
        progress_updated_at: custom "progress_updated_at" => {
            row.try_get::<Option<String>, _>("progress_updated_at")
                .ok()
                .flatten()
                .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
        },
        validation_status: custom "validation_status" => {
            let s = row.try_get::<String, _>("validation_status").unwrap_or_else(|_| "pending".to_string());
            ValidationStatus::from_str(&s).unwrap_or_default()
        },
        validation_evidence: custom "validation_evidence" => {
            let raw = row.try_get::<String, _>("validation_evidence").unwrap_or_else(|_| "[]".to_string());
            serde_json::from_str::<Vec<ValidationEvidence>>(&raw).unwrap_or_default()
        },
        change_rationale: custom "change_rationale" => {
            row.try_get::<String, _>("change_rationale").unwrap_or_default()
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
        let ws = store
            .init_workspace_impl("test", "")
            .await
            .expect("init workspace");
        ws.id
    }

    #[tokio::test]
    async fn plan_crud() {
        let s = store().await;
        let ws = workspace(&s).await;

        let plan = s
            .create_plan_impl(ws, "Add auth endpoint with tests")
            .await
            .expect("create plan");
        assert_eq!(plan.status, PlanStatus::Draft);
        assert_eq!(plan.intent, "Add auth endpoint with tests");

        let fetched = s.get_plan_impl(ws, plan.id).await.expect("get plan");
        assert_eq!(fetched.id, plan.id);

        let updated = s
            .update_plan_status_impl(ws, plan.id, PlanStatus::Approved)
            .await
            .expect("update status");
        assert_eq!(updated.status, PlanStatus::Approved);

        let plans = s
            .list_plans_impl(ws, Some(PlanStatus::Approved))
            .await
            .expect("list plans");
        assert_eq!(plans.len(), 1);
    }

    #[tokio::test]
    async fn work_package_crud_and_readiness() {
        let s = store().await;
        let ws = workspace(&s).await;
        let plan = s
            .create_plan_impl(ws, "test plan")
            .await
            .expect("create plan");

        let wp1 = s
            .create_work_package_impl(ws, Some(plan.id), "first task", vec![], 0, vec![], vec![])
            .await
            .expect("create wp1");
        let wp2 = s
            .create_work_package_impl(
                ws,
                Some(plan.id),
                "second task",
                vec!["Returns 201".to_string()],
                1,
                vec![wp1.id],
                vec![],
            )
            .await
            .expect("create wp2");

        assert_eq!(wp1.status, WorkPackageStatus::Planned);
        assert_eq!(wp2.status, WorkPackageStatus::Planned);

        // Neither WP is ready yet: wp1 has no deps (should become ready), wp2 depends on wp1
        let ready = s
            .refresh_work_package_readiness_impl(ws, plan.id)
            .await
            .expect("refresh readiness");
        // Only wp1 becomes ready (no deps)
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, wp1.id);

        // Mark wp1 done, then wp2 should become ready
        s.update_work_package_status_impl(ws, wp1.id, WorkPackageStatus::Done)
            .await
            .expect("mark wp1 done");

        let ready2 = s
            .refresh_work_package_readiness_impl(ws, plan.id)
            .await
            .expect("refresh readiness 2");
        assert_eq!(ready2.len(), 1);
        assert_eq!(ready2[0].id, wp2.id);
    }

    #[tokio::test]
    async fn assign_work_package() {
        let s = store().await;
        let ws = workspace(&s).await;

        // Need a thread to assign
        let thread = s
            .create_thread_impl(ws, "wp-thread", "", vec![])
            .await
            .expect("create thread");

        let wp = s
            .create_work_package_impl(ws, None, "do something", vec![], 0, vec![], vec![])
            .await
            .expect("create wp");

        let assigned = s
            .assign_work_package_impl(ws, wp.id, "claude", thread.id)
            .await
            .expect("assign");
        assert_eq!(assigned.assigned_agent, Some("claude".to_string()));
        assert_eq!(assigned.thread_id, Some(thread.id));
        assert_eq!(assigned.status, WorkPackageStatus::InProgress);
    }
}
