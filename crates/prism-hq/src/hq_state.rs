use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, Global, Task, WeakEntity};
use uglyhat::model::{
    ActivityEntry, AgentStatus, Handoff, InboxEntry, Plan, PlanStatus, Thread, ThreadStatus,
    WorkPackage, WorkspaceOverview,
};
use uglyhat::store::ActivityFilters;
use uglyhat_panel::UglyhatService;

const REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const ACTIVITY_LIMIT: i64 = 200;

/// Snapshot of uglyhat state, refreshed every 3 seconds.
pub struct HqState {
    pub overview: Option<WorkspaceOverview>,
    pub activity: Vec<ActivityEntry>,
    pub threads: Vec<Thread>,
    pub handoffs: Vec<Handoff>,
    /// Flat agent roster, always available without going through `overview`.
    pub agents: Vec<AgentStatus>,
    /// Unread message counts keyed by agent name; only includes agents with unread > 0.
    pub unread_by_agent: std::collections::HashMap<String, i64>,
    /// Unread supervisory inbox entries (not dismissed).
    pub inbox_entries: Vec<InboxEntry>,
    /// Count of unread inbox entries for badge display.
    pub unread_inbox_count: i64,
    /// Active and approved plans.
    pub plans: Vec<Plan>,
    /// Work packages across all active plans.
    pub work_packages: Vec<WorkPackage>,
    pub is_loading: bool,
    pub error: Option<String>,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
}

/// Newtype wrapper so we can register Entity<HqState> as a GPUI Global
/// without violating the orphan rule.
pub struct HqStateGlobal(pub Entity<HqState>);

impl Global for HqStateGlobal {}

impl HqState {
    /// Initialize HqState, register it as a global, start 3s polling.
    pub fn init_global(cx: &mut App) -> Entity<Self> {
        let state = cx.new(|cx: &mut Context<HqState>| {
            let mut hq = HqState {
                overview: None,
                activity: Vec::new(),
                threads: Vec::new(),
                handoffs: Vec::new(),
                agents: Vec::new(),
                unread_by_agent: std::collections::HashMap::new(),
                inbox_entries: Vec::new(),
                unread_inbox_count: 0,
                plans: Vec::new(),
                work_packages: Vec::new(),
                is_loading: false,
                error: None,
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

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.is_loading = true;
        cx.notify();

        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            // Extract the handle before the await boundary so the borrow is dropped.
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<UglyhatService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            type RefreshData = (
                WorkspaceOverview,
                Vec<ActivityEntry>,
                Vec<Thread>,
                Vec<Handoff>,
                Vec<AgentStatus>,
                std::collections::HashMap<String, i64>,
                Vec<InboxEntry>,
                Vec<Plan>,
                Vec<WorkPackage>,
            );

            let result: anyhow::Result<RefreshData> = cx
                .background_spawn(async move {
                    let Some(handle) = handle else {
                        anyhow::bail!("uglyhat not available");
                    };
                    let overview = handle.get_workspace_overview()?;
                    let activity = handle.list_activity(ActivityFilters {
                        limit: ACTIVITY_LIMIT,
                        ..Default::default()
                    })?;
                    let threads = handle.list_threads(Some(ThreadStatus::Active))?;
                    let handoffs = handle.list_handoffs(None, None)?;
                    let agents = handle.list_agents()?;
                    let unread_by_agent = handle.count_all_unread_messages().unwrap_or_default();
                    let inbox_entries = handle
                        .list_inbox_entries(Default::default())
                        .unwrap_or_default();
                    // Fetch all plans, keep only those worth showing in the UI.
                    let plans = handle
                        .list_plans(None)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|p| {
                            matches!(p.status, PlanStatus::Active | PlanStatus::Approved)
                        })
                        .collect::<Vec<_>>();
                    // Only load WPs for the plans we're actually displaying.
                    let active_plan_ids: std::collections::HashSet<uuid::Uuid> =
                        plans.iter().map(|p| p.id).collect();
                    let work_packages = if active_plan_ids.is_empty() {
                        Vec::new()
                    } else {
                        handle
                            .list_work_packages(None, None)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|wp| {
                                wp.plan_id
                                    .map(|id| active_plan_ids.contains(&id))
                                    .unwrap_or(false)
                            })
                            .collect()
                    };
                    anyhow::Ok((
                        overview,
                        activity,
                        threads,
                        handoffs,
                        agents,
                        unread_by_agent,
                        inbox_entries,
                        plans,
                        work_packages,
                    ))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok((
                        overview,
                        activity,
                        threads,
                        handoffs,
                        agents,
                        unread_by_agent,
                        inbox_entries,
                        plans,
                        work_packages,
                    )) => {
                        this.overview = Some(overview);
                        this.activity = activity;
                        this.threads = threads;
                        this.handoffs = handoffs;
                        this.agents = agents;
                        this.unread_by_agent = unread_by_agent;
                        // Derive unread count from the list rather than a separate DB query.
                        this.unread_inbox_count =
                            inbox_entries.iter().filter(|e| !e.read).count() as i64;
                        this.inbox_entries = inbox_entries;
                        this.plans = plans;
                        this.work_packages = work_packages;
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
