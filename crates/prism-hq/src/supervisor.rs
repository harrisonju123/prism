use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use gpui::AsyncApp;
use prism_context::model::{
    AgentState, InboxEntryType, InboxSeverity, PlanStatus, WorkPackageStatus,
};
use prism_context::scheduler::SchedulerConfig;
use uuid::Uuid;

use crate::agent_spawner::spawn_agent_in_worktree;
use crate::context_service::ContextHandle;
use crate::orchestrator_filter::{FilterVerdict, OrchestratorFilter, ProgressEvent};

pub struct SupervisorConfig {
    pub enabled: bool,
    pub tick_interval: Duration,
    pub max_assignments_per_tick: usize,
    /// Haiku model ID for progress filtering. None disables filtering.
    pub filter_model: Option<String>,
    pub gateway_url: String,
    pub gateway_api_key: Option<String>,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            tick_interval: Duration::from_secs(15),
            max_assignments_per_tick: 3,
            filter_model: None,
            gateway_url: "http://localhost:3000".to_string(),
            gateway_api_key: None,
        }
    }
}

pub struct SupervisorTickResult {
    pub completions_detected: usize,
    pub assignments_made: usize,
    pub events_filtered: usize,
    pub plans_completed: usize,
}

pub async fn supervisor_tick(
    handle: &ContextHandle,
    config: &SupervisorConfig,
    filter: &Option<OrchestratorFilter>,
    progress_watermarks: &mut HashMap<Uuid, DateTime<Utc>>,
    repo_root: PathBuf,
    cx: &mut AsyncApp,
) -> Result<SupervisorTickResult> {
    let mut result = SupervisorTickResult {
        completions_detected: 0,
        assignments_made: 0,
        events_filtered: 0,
        plans_completed: 0,
    };

    // 1. Reap timed-out handoffs
    if let Err(e) = handle.reap_timed_out_handoffs() {
        log::warn!("supervisor: reap handoffs failed: {e}");
    }

    // 2. Detect completions — InProgress WPs whose assigned agent is now Idle+closed
    let in_progress_wps =
        handle.list_work_packages(None, Some(WorkPackageStatus::InProgress))?;
    let agents = handle.list_agents()?;

    for wp in &in_progress_wps {
        if let Some(ref agent_name) = wp.assigned_agent {
            let is_done = agents.iter().any(|a| {
                a.name == *agent_name
                    && a.state == AgentState::Idle
                    && !a.session_open
            });
            if is_done {
                if let Err(e) =
                    handle.update_work_package_status(wp.id, WorkPackageStatus::Done)
                {
                    log::warn!("supervisor: failed to mark WP {} done: {e}", wp.id);
                    continue;
                }
                result.completions_detected += 1;
                if let Some(plan_id) = wp.plan_id {
                    let _ = handle.refresh_work_package_readiness(plan_id);
                }
            }
        }
    }

    // 3. Plan completion — all WPs done → mark plan Completed
    let active_plans = handle.list_plans(Some(PlanStatus::Active))?;
    for plan in &active_plans {
        let all_wps = handle.list_work_packages(Some(plan.id), None)?;
        if !all_wps.is_empty()
            && all_wps
                .iter()
                .all(|wp| wp.status == WorkPackageStatus::Done)
        {
            let _ = handle.update_plan_status(plan.id, PlanStatus::Completed);
            let _ = handle.create_inbox_entry(
                InboxEntryType::Completed,
                &format!("Plan completed: {}", plan.intent),
                "All work packages finished successfully.",
                InboxSeverity::Info,
                Some("supervisor"),
                Some("plan"),
                Some(plan.id),
            );
            result.plans_completed += 1;
        }
    }

    // 4–6. Compute assignments, commit, spawn agents
    let scheduler_config = SchedulerConfig {
        max_assignments_per_tick: config.max_assignments_per_tick,
    };
    let assignments = handle.compute_assignments(&scheduler_config)?;

    for assignment in assignments {
        let wp = &assignment.work_package;

        let thread = match handle.create_thread(&assignment.thread_name, &wp.intent, wp.tags.clone())
        {
            Ok(t) => t,
            Err(e) => {
                log::warn!("supervisor: create_thread for WP {} failed: {e}", wp.id);
                continue;
            }
        };

        if let Err(e) = handle.assign_work_package(wp.id, &assignment.agent_name, thread.id) {
            log::warn!("supervisor: assign WP {} failed: {e}", wp.id);
            continue;
        }

        // Send startup message with intent + acceptance criteria
        let criteria = if wp.acceptance_criteria.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nAcceptance criteria:\n{}",
                wp.acceptance_criteria
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}. {c}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        let startup_msg = format!(
            "You have been assigned work package: {}{}\n\nThread: {}",
            wp.intent, criteria, assignment.thread_name
        );
        let _ = handle.send_message("supervisor", &assignment.agent_name, &startup_msg);

        let agent_name = assignment.agent_name.clone();
        match spawn_agent_in_worktree(agent_name.clone(), repo_root.clone(), cx).await {
            Ok(()) => {
                log::info!("supervisor: spawned agent {agent_name} for WP {}", wp.id);
                result.assignments_made += 1;
            }
            Err(e) => {
                log::warn!("supervisor: spawn agent {agent_name} failed: {e}");
            }
        }
    }

    // 7. Progress filtering — poll fresh progress notes
    if let Some(filter) = filter {
        let now = Utc::now();
        let current_wps =
            handle.list_work_packages(None, Some(WorkPackageStatus::InProgress))?;

        for wp in &current_wps {
            if let (Some(note), Some(updated_at)) = (&wp.progress_note, wp.progress_updated_at) {
                let watermark = progress_watermarks
                    .get(&wp.id)
                    .copied()
                    .unwrap_or(DateTime::<Utc>::MIN_UTC);
                if updated_at > watermark {
                    let event = ProgressEvent {
                        wp_id: wp.id,
                        wp_intent: wp.intent.clone(),
                        agent_name: wp.assigned_agent.clone().unwrap_or_default(),
                        progress_note: note.clone(),
                        wp_status: wp.status.clone(),
                    };

                    let verdict = filter.classify(&event).await;
                    result.events_filtered += 1;
                    progress_watermarks.insert(wp.id, now);

                    match verdict {
                        FilterVerdict::Silent => {}
                        FilterVerdict::Escalate { summary, options } => {
                            let _ = handle.create_inbox_entry(
                                InboxEntryType::Approval,
                                &summary,
                                &options.join("\n"),
                                InboxSeverity::Warning,
                                wp.assigned_agent.as_deref(),
                                Some("work_package"),
                                Some(wp.id),
                            );
                        }
                        FilterVerdict::Relay { to_agent, message } => {
                            let _ = handle.send_message("supervisor", &to_agent, &message);
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}
