use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task, WeakEntity};
use prism_context::model::{AgentStatus, InboxEntry, InboxEntryType, InboxSeverity, Plan, PlanStatus, Risk};
use crate::context_service::ContextService;
use crate::running_agents::RunningAgents;

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
    /// Entry IDs already seen by the OS-notification logic (prevents re-firing on re-poll).
    seen_entry_ids: std::collections::HashSet<uuid::Uuid>,
    /// Agent names for which we've already fired a completion OS notification.
    notified_completions: std::collections::HashSet<String>,
    /// Rate-limit OS notifications to at most 1 per 10 seconds.
    last_os_notification: Option<Instant>,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
}

/// Newtype wrapper so we can register Entity<HqState> as a GPUI Global
/// without violating the orphan rule.
pub struct HqStateGlobal(pub Entity<HqState>);

impl Global for HqStateGlobal {}

impl HqState {
    /// Initialize HqState, register it as a global, start 10s polling.
    pub fn init_global(cx: &mut App) -> Entity<Self> {
        let state = cx.new(|cx: &mut Context<HqState>| {
            let mut hq = HqState {
                agents: Vec::new(),
                inbox_entries: Vec::new(),
                risks: Vec::new(),
                high_risk_count: 0,
                active_plan: None,
                is_loading: false,
                error: None,
                seen_entry_ids: std::collections::HashSet::new(),
                notified_completions: std::collections::HashSet::new(),
                last_os_notification: None,
                refresh_task: None,
                _auto_refresh: Task::ready(()),
            };

            let auto_refresh = cx.spawn(async move |this: WeakEntity<HqState>, cx| {
                loop {
                    cx.background_executor().timer(REFRESH_INTERVAL).await;
                    this.update(cx, |hq, cx| hq.refresh(cx)).ok();
                }
            });
            hq._auto_refresh = auto_refresh;
            hq.refresh(cx);
            hq
        });

        cx.set_global(HqStateGlobal(state.clone()));
        state
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
