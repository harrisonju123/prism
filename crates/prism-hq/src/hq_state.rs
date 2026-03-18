use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task, WeakEntity};
use prism_context::model::{AgentStatus, InboxEntry, InboxEntryType, InboxSeverity, Plan, PlanStatus, Risk};
use uuid::Uuid;

use crate::context_service::ContextService;
use crate::orchestrator_filter::OrchestratorFilter;
use crate::running_agents::RunningAgents;
use crate::supervisor::{SupervisorConfig, supervisor_tick};

const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// Snapshot of prism context state, refreshed every 10 seconds.
pub struct HqState {
    /// Flat agent roster.
    pub agents: Vec<AgentStatus>,
    /// Unread supervisory inbox entries (not dismissed).
    pub inbox_entries: Vec<InboxEntry>,
    /// Open/unverified risks from the risk register.
    pub risks: Vec<Risk>,
    /// Count of High-severity unverified risks (for status indicator badge).
    pub high_risk_count: usize,
    /// Active (or approved) plan, if any.
    pub active_plan: Option<Plan>,
    pub is_loading: bool,
    pub error: Option<String>,
    /// Cumulative cost across active plan sessions (USD).
    pub cumulative_cost_usd: f64,
    /// Entry IDs already seen by the OS-notification logic (prevents re-firing on re-poll).
    seen_entry_ids: std::collections::HashSet<Uuid>,
    /// Agent names for which we've already fired a completion OS notification.
    notified_completions: std::collections::HashSet<String>,
    /// Rate-limit OS notifications to at most 1 per 10 seconds.
    last_os_notification: Option<Instant>,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
    /// Supervisor loop configuration.
    pub supervisor_config: SupervisorConfig,
    /// Per-WP watermarks for progress filtering (wp_id → last_seen timestamp).
    progress_watermarks: HashMap<Uuid, chrono::DateTime<chrono::Utc>>,
    _supervisor_task: Task<()>,
}

/// Newtype wrapper so we can register Entity<HqState> as a GPUI Global
/// without violating the orphan rule.
pub struct HqStateGlobal(pub Entity<HqState>);

impl Global for HqStateGlobal {}

impl HqState {
    /// Initialize HqState, register it as a global, start 10s polling.
    pub fn init_global(cx: &mut App) -> Entity<Self> {
        let state = cx.new(|cx: &mut Context<HqState>| {
            let auto_refresh = cx.spawn(async move |this: WeakEntity<HqState>, cx| {
                loop {
                    cx.background_executor().timer(REFRESH_INTERVAL).await;
                    this.update(cx, |hq, cx| hq.refresh(cx)).ok();
                }
            });

            let supervisor_task = cx.spawn(async move |this: WeakEntity<HqState>, cx| {
                loop {
                    // Sleep first using the configured interval
                    let interval = this
                        .update(cx, |hq, _| hq.supervisor_config.tick_interval)
                        .unwrap_or(Duration::from_secs(15));
                    cx.background_executor().timer(interval).await;

                    // Gather everything needed for the tick in one sync update
                    type TickInputs = (
                        crate::context_service::ContextHandle,
                        SupervisorConfig,
                        Option<OrchestratorFilter>,
                        HashMap<Uuid, chrono::DateTime<chrono::Utc>>,
                        PathBuf,
                    );
                    let inputs: Option<TickInputs> = this
                        .update(cx, |hq, cx| {
                            if !hq.supervisor_config.enabled {
                                return None;
                            }
                            let handle = cx
                                .try_global::<ContextService>()
                                .and_then(|svc| svc.handle())?;
                            let config = SupervisorConfig {
                                enabled: hq.supervisor_config.enabled,
                                tick_interval: hq.supervisor_config.tick_interval,
                                max_assignments_per_tick: hq
                                    .supervisor_config
                                    .max_assignments_per_tick,
                                filter_model: hq.supervisor_config.filter_model.clone(),
                                gateway_url: hq.supervisor_config.gateway_url.clone(),
                                gateway_api_key: hq.supervisor_config.gateway_api_key.clone(),
                            };
                            let filter =
                                config.filter_model.as_ref().map(|model| {
                                    OrchestratorFilter::new(
                                        config.gateway_url.clone(),
                                        config.gateway_api_key.clone(),
                                        model.clone(),
                                    )
                                });
                            let watermarks = hq.progress_watermarks.clone();
                            let repo_root = std::env::current_dir()
                                .unwrap_or_else(|_| PathBuf::from("."));
                            Some((handle, config, filter, watermarks, repo_root))
                        })
                        .ok()
                        .flatten();

                    let Some((handle, config, filter, mut watermarks, repo_root)) = inputs else {
                        continue;
                    };

                    match supervisor_tick(
                        &handle,
                        &config,
                        &filter,
                        &mut watermarks,
                        repo_root,
                        cx,
                    )
                    .await
                    {
                        Ok(tick_result) => {
                            if tick_result.completions_detected > 0
                                || tick_result.assignments_made > 0
                                || tick_result.plans_completed > 0
                            {
                                log::info!(
                                    "supervisor tick: {} completions, {} assignments, {} plans done, {} filtered",
                                    tick_result.completions_detected,
                                    tick_result.assignments_made,
                                    tick_result.plans_completed,
                                    tick_result.events_filtered,
                                );
                            }
                            this.update(cx, |hq, _| {
                                hq.progress_watermarks = watermarks;
                            })
                            .ok();
                        }
                        Err(e) => log::warn!("supervisor tick error: {e}"),
                    }
                }
            });

            let mut hq = HqState {
                agents: Vec::new(),
                inbox_entries: Vec::new(),
                risks: Vec::new(),
                high_risk_count: 0,
                active_plan: None,
                is_loading: false,
                error: None,
                cumulative_cost_usd: 0.0,
                seen_entry_ids: std::collections::HashSet::new(),
                notified_completions: std::collections::HashSet::new(),
                last_os_notification: None,
                refresh_task: None,
                _auto_refresh: auto_refresh,
                supervisor_config: SupervisorConfig::default(),
                progress_watermarks: HashMap::new(),
                _supervisor_task: supervisor_task,
            };
            hq.refresh(cx);
            hq
        });

        cx.set_global(HqStateGlobal(state.clone()));
        state
    }

    /// Enable or disable the supervisor loop.
    pub fn set_supervisor_enabled(&mut self, enabled: bool) {
        self.supervisor_config.enabled = enabled;
    }

    /// Get the globally registered HqState entity, if any.
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<HqStateGlobal>().map(|g| g.0.clone())
    }

    /// Returns the active (or approved) plan, if any.
    pub fn active_plan(&self) -> Option<&Plan> {
        self.active_plan.as_ref()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.is_loading = true;
        cx.notify();

        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<ContextService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            type RefreshData = (Vec<AgentStatus>, Vec<InboxEntry>, Vec<Risk>, Option<Plan>);

            let result: anyhow::Result<RefreshData> = cx
                .background_spawn(async move {
                    let Some(handle) = handle else {
                        anyhow::bail!("context service not available");
                    };
                    let agents = handle.list_agents()?;
                    // Auto-dismiss stale Completed entries (older than 24 hours).
                    let _ = handle.dismiss_expired_entries(InboxEntryType::Completed, 86400);
                    let inbox_entries = handle
                        .list_inbox_entries(Default::default())
                        .unwrap_or_default();
                    let risks = handle.list_unverified_risks(None).unwrap_or_default();
                    let active_plan = handle.list_plans(Some(PlanStatus::Active)).ok()
                        .and_then(|plans| plans.into_iter().next())
                        .or_else(|| handle.list_plans(Some(PlanStatus::Approved)).ok()
                            .and_then(|plans| plans.into_iter().next()));
                    anyhow::Ok((agents, inbox_entries, risks, active_plan))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok((agents, inbox_entries, risks, active_plan)) => {
                        this.agents = agents;
                        this.active_plan = active_plan;

                        // Fire OS notifications for new Critical unread entries.
                        let now = Instant::now();
                        let can_notify = this
                            .last_os_notification
                            .map(|t| now.duration_since(t) >= Duration::from_secs(10))
                            .unwrap_or(true);
                        for entry in &inbox_entries {
                            if !this.seen_entry_ids.contains(&entry.id)
                                && entry.severity == InboxSeverity::Critical
                                && !entry.read
                                && can_notify
                            {
                                crate::notification::notify_os("PrisM", &entry.title);
                                this.last_os_notification = Some(now);
                                break;
                            }
                        }
                        for entry in &inbox_entries {
                            this.seen_entry_ids.insert(entry.id);
                        }

                        // Fire OS notifications for Completed entries from locally-spawned agents.
                        let running_agents = RunningAgents::global(cx);
                        for entry in &inbox_entries {
                            let InboxEntryType::Completed = entry.entry_type else { continue };
                            let Some(ref source) = entry.source_agent else { continue };
                            if this.notified_completions.contains(source) {
                                continue;
                            }
                            let is_local = running_agents
                                .as_ref()
                                .map(|ra| ra.read(cx).was_spawned(source))
                                .unwrap_or(false);
                            if is_local {
                                crate::notification::notify_os(
                                    "PrisM",
                                    &format!("{} finished", source),
                                );
                                this.notified_completions.insert(source.clone());
                            }
                        }

                        this.inbox_entries = inbox_entries;
                        this.high_risk_count = risks
                            .iter()
                            .filter(|r| r.severity == prism_context::model::RiskSeverity::High)
                            .count();
                        this.risks = risks;
                        this.error = None;
                    }
                    Err(e) => {
                        this.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }
}
