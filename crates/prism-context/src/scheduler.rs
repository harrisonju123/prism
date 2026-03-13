use chrono::Utc;
use uuid::Uuid;

use crate::error::Result;
use crate::model::{AgentState, Handoff, HandoffStatus, PlanStatus, WorkPackage, WorkPackageStatus};
use crate::store::Store;

pub struct SchedulerConfig {
    pub max_assignments_per_tick: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_assignments_per_tick: 5,
        }
    }
}

/// A computed assignment instruction. The caller is responsible for committing it
/// (creating the thread and calling assign_work_package) and spawning the agent process.
pub struct Assignment {
    pub work_package: WorkPackage,
    pub agent_name: String,
    pub thread_name: String,
}

/// Compute which ready work packages should be assigned this tick.
///
/// Mutates store state only via `refresh_work_package_readiness` (Planned→Ready).
/// Does NOT call `assign_work_package` — the caller commits assignments atomically
/// alongside thread creation and agent spawning.
pub async fn compute_assignments(
    store: &dyn Store,
    workspace_id: Uuid,
    config: &SchedulerConfig,
) -> Result<Vec<Assignment>> {
    // Refresh readiness for every active plan before picking work
    let active_plans = store
        .list_plans(workspace_id, Some(PlanStatus::Active))
        .await?;
    for plan in &active_plans {
        store
            .refresh_work_package_readiness(workspace_id, plan.id)
            .await?;
    }

    // All unassigned Ready WPs across the workspace
    let ready_wps = store
        .list_work_packages(workspace_id, None, Some(WorkPackageStatus::Ready))
        .await?;
    let mut unassigned: Vec<WorkPackage> = ready_wps
        .into_iter()
        .filter(|wp| wp.assigned_agent.is_none())
        .collect();

    if unassigned.is_empty() {
        return Ok(vec![]);
    }

    // Lower ordinal = higher priority
    unassigned.sort_by_key(|wp| wp.ordinal);

    // Idle agents with no open session are available for immediate assignment
    let agents = store.list_agents(workspace_id).await?;
    let available: Vec<String> = agents
        .iter()
        .filter(|a| a.state == AgentState::Idle && !a.session_open)
        .map(|a| a.name.clone())
        .collect();

    let ts = chrono::Utc::now().timestamp_millis();
    let mut assignments = Vec::new();

    for (i, wp) in unassigned.into_iter().enumerate() {
        if assignments.len() >= config.max_assignments_per_tick {
            break;
        }
        // Reuse an idle agent if one is available; otherwise generate a fresh name for spawning
        let agent_name = if i < available.len() {
            available[i].clone()
        } else {
            format!("wp-agent-{ts}-{i}")
        };
        let thread_name = slugify(&wp.intent);
        assignments.push(Assignment {
            work_package: wp,
            agent_name,
            thread_name,
        });
    }

    Ok(assignments)
}

/// Reap timed-out handoffs by transitioning them to `Failed`.
///
/// Checks both `Running` handoffs (using `started_at`) and `Accepted` handoffs
/// (using `updated_at` as the acceptance time).
pub async fn reap_timed_out_handoffs(
    store: &dyn Store,
    workspace_id: Uuid,
) -> Result<Vec<Handoff>> {
    let now = Utc::now();
    let mut reaped = Vec::new();

    let (running, accepted) = tokio::try_join!(
        store.list_handoffs(workspace_id, None, Some(HandoffStatus::Running)),
        store.list_handoffs(workspace_id, None, Some(HandoffStatus::Accepted)),
    )?;

    // Running handoffs: time from started_at
    for h in running {
        if let Some(timeout_secs) = h.constraints.timeout_secs {
            let start = h.started_at.unwrap_or(h.updated_at);
            if (now - start).num_seconds() >= timeout_secs as i64 {
                if let Ok(failed) = store.fail_handoff(workspace_id, h.id, "timed out").await {
                    reaped.push(failed);
                }
            }
        }
    }

    // Accepted handoffs: time from updated_at (acceptance time)
    for h in accepted {
        if let Some(timeout_secs) = h.constraints.timeout_secs {
            if (now - h.updated_at).num_seconds() >= timeout_secs as i64 {
                if let Ok(failed) = store.fail_handoff(workspace_id, h.id, "timed out").await {
                    reaped.push(failed);
                }
            }
        }
    }

    Ok(reaped)
}

/// Convert free-form text to a URL-safe slug (max 60 chars).
/// Duplicated from prism-hq to keep prism-context dependency-free.
pub fn slugify(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut slug = String::with_capacity(lower.len());
    let mut last_was_dash = false;
    for c in lower.chars() {
        if c.is_alphanumeric() {
            slug.push(c);
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }
    if slug.ends_with('-') {
        slug.pop();
    }
    slug.truncate(60);
    slug
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;

    #[tokio::test]
    async fn test_compute_assignments_respects_deps() {
        let store = SqliteStore::open_memory().await.expect("open in-memory db");

        let ws = store.init_workspace("test", "").await.unwrap();
        let plan = store.create_plan(ws.id, "test plan").await.unwrap();
        store
            .update_plan_status(ws.id, plan.id, PlanStatus::Active)
            .await
            .unwrap();

        // WP1: no deps → becomes Ready after first refresh
        let wp1 = store
            .create_work_package(ws.id, Some(plan.id), "task one", vec![], 1, vec![], vec![])
            .await
            .unwrap();
        // WP2 depends on WP1
        let wp2 = store
            .create_work_package(
                ws.id,
                Some(plan.id),
                "task two",
                vec![],
                2,
                vec![wp1.id],
                vec![],
            )
            .await
            .unwrap();
        // WP3 depends on WP2
        let _wp3 = store
            .create_work_package(
                ws.id,
                Some(plan.id),
                "task three",
                vec![],
                3,
                vec![wp2.id],
                vec![],
            )
            .await
            .unwrap();

        let config = SchedulerConfig::default();

        // Only WP1 is unblocked at this point
        let assignments = compute_assignments(&store, ws.id, &config).await.unwrap();
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].work_package.id, wp1.id);

        // Complete WP1 → WP2 should unlock next tick
        store
            .update_work_package_status(ws.id, wp1.id, WorkPackageStatus::Done)
            .await
            .unwrap();

        let assignments2 = compute_assignments(&store, ws.id, &config).await.unwrap();
        assert_eq!(assignments2.len(), 1);
        assert_eq!(assignments2[0].work_package.id, wp2.id);
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Implement OAuth2 flow"), "implement-oauth2-flow");
        assert_eq!(slugify("  leading spaces"), "leading-spaces");
        assert_eq!(slugify("foo--bar!!baz"), "foo-bar-baz");
        assert_eq!(slugify("already-slug"), "already-slug");
    }
}
