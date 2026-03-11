use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, Global, Task, WeakEntity};
use uglyhat::model::{ActivityEntry, AgentStatus, Handoff, Thread, ThreadStatus, WorkspaceOverview};
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
                is_loading: false,
                error: None,
                refresh_task: None,
                _auto_refresh: Task::ready(()),
            };

            let auto_refresh = cx.spawn(async move |this: WeakEntity<HqState>, cx| loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                this.update(cx, |hq, cx| hq.refresh(cx)).ok();
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

            let result: anyhow::Result<(WorkspaceOverview, Vec<ActivityEntry>, Vec<Thread>, Vec<Handoff>, Vec<AgentStatus>, std::collections::HashMap<String, i64>)> = cx
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
                    anyhow::Ok((overview, activity, threads, handoffs, agents, unread_by_agent))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok((overview, activity, threads, handoffs, agents, unread_by_agent)) => {
                        this.overview = Some(overview);
                        this.activity = activity;
                        this.threads = threads;
                        this.handoffs = handoffs;
                        this.agents = agents;
                        this.unread_by_agent = unread_by_agent;
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
