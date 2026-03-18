use std::collections::{HashMap, HashSet};

use gpui::{
    Action, App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, Render,
    SharedString, Task, WeakEntity, Window, actions, px,
};
use prism_context::model::{
    AgentStatus, AssumptionStatus, BlockerStatus, ChangeSet, Decision, Memory,
    Plan, Thread, WorkPackage, WorkPackageStatus,
};
use prism_context::store::MemoryFilters;
use schemars::JsonSchema;
use serde::Deserialize;
use ui::{
    Button, ButtonStyle, Color, Disclosure, Divider, DividerColor,
    Icon, IconName, Label, LabelSize, prelude::*, v_flex, h_flex,
};
use uuid::Uuid;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::context_service::ContextService;
use crate::hq_state::HqState;
use crate::activity_bus;

actions!(prism_hq, [TogglePlansPanel]);

/// Navigate to the agent chat session associated with a context thread ID.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = prism_hq)]
pub struct OpenAgentChatSession {
    pub context_thread_id: String,
    pub title: Option<String>,
}

const PLANS_PANEL_KEY: &str = "prism_plans_panel";

struct PlanDetail {
    work_packages: Vec<WorkPackage>,
    change_sets: Vec<ChangeSet>,
    is_loading: bool,
}

pub struct PlansPanel {
    focus_handle: FocusHandle,
    _hq_subscription: Option<gpui::Subscription>,
    _activity_subscription: Option<gpui::Subscription>,
    position: DockPosition,
    width: Option<gpui::Pixels>,
    // Plans section
    plans: Vec<Plan>,
    expanded_plans: HashSet<Uuid>,
    plan_details: HashMap<Uuid, PlanDetail>,
    // Context section (memories, decisions, agents, threads)
    memories: Vec<Memory>,
    decisions: Vec<Decision>,
    agents: Vec<AgentStatus>,
    threads: Vec<Thread>,
    current_thread_title: Option<String>,
    // Shared state
    is_loading: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
}

impl EventEmitter<PanelEvent> for PlansPanel {}

impl PlansPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let auto_refresh = cx.spawn(async move |this: WeakEntity<PlansPanel>, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                this.update(cx, |panel, cx| panel.refresh(cx)).ok();
            }
        });

        let hq_subscription = HqState::global(cx).map(|hq_entity| {
            cx.observe(&hq_entity, |this, hq, cx| {
                let agents = hq.read(cx).agents.clone();
                this.agents = agents;
                if let Some(plan) = hq.read(cx).active_plan() {
                    // Update the plan in our list if it changed
                    if let Some(existing) = this.plans.iter_mut().find(|p| p.id == plan.id) {
                        *existing = plan.clone();
                    }
                }
                cx.notify();
            })
        });

        let activity_subscription = activity_bus::global_inner(cx)
            .map(|bus_entity| {
                cx.observe(&bus_entity, |this, bus, cx| {
                    this.current_thread_title = bus.read(cx).thread_title.clone();
                    cx.notify();
                })
            });

        let mut panel = Self {
            focus_handle,
            _hq_subscription: hq_subscription,
            _activity_subscription: activity_subscription,
            position: DockPosition::Right,
            width: None,
            plans: Vec::new(),
            expanded_plans: HashSet::new(),
            plan_details: HashMap::new(),
            memories: Vec::new(),
            decisions: Vec::new(),
            agents: Vec::new(),
            threads: Vec::new(),
            current_thread_title: None,
            is_loading: false,
            error: None,
            refresh_task: None,
            _auto_refresh: auto_refresh,
        };
        panel.refresh(cx);
        panel
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

            let result: anyhow::Result<(
                Vec<Plan>,
                Vec<Memory>,
                Vec<Decision>,
                Vec<Thread>,
            )> = cx
                .background_spawn(async move {
                    let handle = handle
                        .ok_or_else(|| anyhow::anyhow!("context service not available"))?;
                    let plans = handle.list_plans(None)?;
                    let memories = handle.load_memories(MemoryFilters::default())?;
                    let decisions = handle.list_decisions(None, None)?;
                    let threads = handle.list_threads(None)?;
                    anyhow::Ok((plans, memories, decisions, threads))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok((plans, memories, decisions, threads)) => {
                        this.plans = plans;
                        this.memories = memories;
                        this.decisions = decisions;
                        this.threads = threads;
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

    fn load_plan_detail(&mut self, plan_id: Uuid, cx: &mut Context<Self>) {
        if let Some(detail) = self.plan_details.get_mut(&plan_id) {
            if detail.is_loading {
                return;
            }
            detail.is_loading = true;
        } else {
            self.plan_details.insert(plan_id, PlanDetail {
                work_packages: Vec::new(),
                change_sets: Vec::new(),
                is_loading: true,
            });
        }
        cx.notify();

        cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<ContextService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<(Vec<WorkPackage>, Vec<ChangeSet>)> = cx
                .background_spawn(async move {
                    let handle = handle
                        .ok_or_else(|| anyhow::anyhow!("context service not available"))?;
                    let wps = handle.list_work_packages(Some(plan_id), None)?;
                    let css = handle.list_change_sets(Some(plan_id), None)?;
                    anyhow::Ok((wps, css))
                })
                .await;

            this.update(cx, |this, cx| {
                if let Some(detail) = this.plan_details.get_mut(&plan_id) {
                    detail.is_loading = false;
                    if let Ok((wps, css)) = result {
                        detail.work_packages = wps;
                        detail.change_sets = css;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for PlansPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PlansPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_loading = self.is_loading;
        let error = self.error.clone();

        let mut content = v_flex().flex_1().overflow_hidden().p_2().gap_3();

        if is_loading && self.plans.is_empty() {
            content = content.child(
                Label::new("Loading…").size(LabelSize::Small).color(Color::Muted),
            );
        }

        if let Some(err) = error {
            content = content.child(
                Label::new(format!("Error: {err}"))
                    .size(LabelSize::Small)
                    .color(Color::Error),
            );
        }

        // ── Plans section ──────────────────────────────────────────────────
        if !self.plans.is_empty() {
            content = content.child(
                Label::new("Plans").size(LabelSize::Small).color(Color::Muted),
            );

            for plan in &self.plans {
                let plan_id = plan.id;
                let is_expanded = self.expanded_plans.contains(&plan_id);
                let detail = self.plan_details.get(&plan_id);

                let status_color = match plan.status.to_string().as_str() {
                    "Active" => Color::Accent,
                    "Completed" => Color::Success,
                    "Cancelled" => Color::Muted,
                    _ => Color::Default,
                };

                let disc_id = format!("plan_disc_{}", plan_id);
                let plan_block = v_flex()
                    .gap_1()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .rounded_md()
                    .p_2()
                    .child(
                        h_flex()
                            .id(SharedString::from(format!("plan_header_{}", plan_id)))
                            .gap_1()
                            .cursor_pointer()
                            .child(
                                Disclosure::new(SharedString::from(disc_id), is_expanded),
                            )
                            .child(
                                Label::new(plan.intent.chars().take(55).collect::<String>())
                                    .size(LabelSize::Small),
                            )
                            .child(gpui::div().flex_1())
                            .child(
                                Label::new(plan.status.to_string())
                                    .size(LabelSize::Small)
                                    .color(status_color),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.expanded_plans.contains(&plan_id) {
                                    this.expanded_plans.remove(&plan_id);
                                } else {
                                    this.expanded_plans.insert(plan_id);
                                    this.load_plan_detail(plan_id, cx);
                                }
                                cx.notify();
                            })),
                    );

                let plan_block = if is_expanded {
                    let phases = prism_context::model::MissionPhase::all();
                    let current_phase_str = plan.current_phase.to_string();

                    let mut detail_content = v_flex().gap_1().pl_4();

                    // Phase
                    detail_content = detail_content.child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(phases.iter().map(|p| {
                                let is_current = *p == current_phase_str.as_str();
                                Label::new(*p)
                                    .size(LabelSize::Small)
                                    .color(if is_current { Color::Accent } else { Color::Muted })
                            })),
                    );

                    // Assumptions / blockers warning
                    let open_blockers = plan.blockers.iter().filter(|b| b.status == BlockerStatus::Open).count();
                    let unverified = plan.assumptions.iter().filter(|a| a.status == AssumptionStatus::Unverified).count();
                    if open_blockers > 0 || unverified > 0 {
                        let mut warnings = Vec::new();
                        if open_blockers > 0 {
                            warnings.push(format!("{open_blockers} blocker{}", if open_blockers == 1 { "" } else { "s" }));
                        }
                        if unverified > 0 {
                            warnings.push(format!("{unverified} unverified"));
                        }
                        detail_content = detail_content.child(
                            Label::new(format!("⚠ {}", warnings.join(" · ")))
                                .size(LabelSize::Small)
                                .color(Color::Warning),
                        );
                    }

                    // Work packages
                    if let Some(detail) = detail {
                        if detail.is_loading {
                            detail_content = detail_content.child(
                                Label::new("Loading…").size(LabelSize::Small).color(Color::Muted),
                            );
                        } else {
                            let done = detail.work_packages.iter().filter(|w| w.status == WorkPackageStatus::Done).count();
                            let total = detail.work_packages.len();
                            if total > 0 {
                                detail_content = detail_content.child(
                                    h_flex().gap_1()
                                        .child(Label::new("WPs:").size(LabelSize::Small).color(Color::Muted))
                                        .child(Label::new(format!("{done}/{total}")).size(LabelSize::Small)),
                                );
                            }
                            for wp in &detail.work_packages {
                                let wp_status_color = match wp.status {
                                    WorkPackageStatus::Done => Color::Success,
                                    WorkPackageStatus::InProgress => Color::Accent,
                                    WorkPackageStatus::Cancelled => Color::Muted,
                                    _ => Color::Default,
                                };
                                let wp_id = wp.id;
                                let status_label = Label::new(format!("[{}]", wp.status))
                                    .size(LabelSize::Small)
                                    .color(wp_status_color);
                                let intent_label = Label::new(
                                    wp.intent.chars().take(45).collect::<String>(),
                                )
                                .size(LabelSize::Small);
                                let base = h_flex()
                                    .gap_1()
                                    .child(status_label)
                                    .child(intent_label);
                                let wp_row: gpui::AnyElement = if let Some(thread_id) = wp.thread_id {
                                    let ctx_id = thread_id.to_string();
                                    let title = wp.intent.clone();
                                    base.id(SharedString::from(format!("wp_{}", wp_id)))
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |_, _, window, cx| {
                                            window.dispatch_action(OpenAgentChatSession {
                                                context_thread_id: ctx_id.clone(),
                                                title: Some(title.clone()),
                                            }.boxed_clone(), cx);
                                        }))
                                        .into_any_element()
                                } else {
                                    base.into_any_element()
                                };
                                detail_content = detail_content.child(wp_row);
                            }

                            // Change sets
                            if !detail.change_sets.is_empty() {
                                detail_content = detail_content.child(
                                    Label::new(format!("Files changed ({})", detail.change_sets.len()))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                );
                                for cs in detail.change_sets.iter().take(8) {
                                    detail_content = detail_content.child(
                                        Label::new(
                                            std::path::Path::new(&cs.file_path)
                                                .file_name()
                                                .and_then(|n| n.to_str())
                                                .unwrap_or(&cs.file_path)
                                                .to_string(),
                                        )
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                    );
                                }
                            }
                        }
                    }

                    plan_block.child(detail_content)
                } else {
                    plan_block
                };

                content = content.child(plan_block);
            }
        } else if !is_loading {
            content = content.child(
                Label::new("No plans yet").size(LabelSize::Small).color(Color::Muted),
            );
        }

        // ── Active agents ──────────────────────────────────────────────────
        if !self.agents.is_empty() {
            content = content.child(
                Divider::horizontal().color(DividerColor::Border),
            );
            content = content.child(
                Label::new("Agents").size(LabelSize::Small).color(Color::Muted),
            );
            for agent in &self.agents {
                let status_color = if agent.session_open {
                    Color::Accent
                } else {
                    Color::Muted
                };
                content = content.child(
                    h_flex().gap_1()
                        .child(Icon::new(IconName::Person).size(ui::IconSize::Small).color(status_color))
                        .child(Label::new(agent.name.clone()).size(LabelSize::Small)),
                );
            }
        }

        // ── Recent memories ────────────────────────────────────────────────
        if !self.memories.is_empty() {
            content = content.child(
                Divider::horizontal().color(DividerColor::Border),
            );
            content = content.child(
                h_flex().gap_1()
                    .child(Label::new("Memories").size(LabelSize::Small).color(Color::Muted))
                    .child(
                        Label::new(format!("({})", self.memories.len()))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            );
            for memory in self.memories.iter().take(5) {
                content = content.child(
                    v_flex()
                        .child(
                            Label::new(memory.key.chars().take(40).collect::<String>())
                                .size(LabelSize::Small)
                                .color(Color::Accent),
                        )
                        .child(
                            Label::new(memory.value.chars().take(80).collect::<String>())
                                .size(LabelSize::Small),
                        ),
                );
            }
        }

        // ── Recent decisions ───────────────────────────────────────────────
        if !self.decisions.is_empty() {
            content = content.child(
                Divider::horizontal().color(DividerColor::Border),
            );
            content = content.child(
                h_flex().gap_1()
                    .child(Label::new("Decisions").size(LabelSize::Small).color(Color::Muted))
                    .child(
                        Label::new(format!("({})", self.decisions.len()))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            );
            for decision in self.decisions.iter().take(5) {
                content = content.child(
                    Label::new(decision.title.chars().take(55).collect::<String>())
                        .size(LabelSize::Small),
                );
            }
        }

        v_flex()
            .size_full()
            .overflow_hidden()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(Label::new("Plans").size(LabelSize::Small).color(Color::Muted))
                    .when(self.current_thread_title.is_some(), |this| {
                        this.child(
                            Label::new(
                                format!("· {}", self.current_thread_title.as_deref().unwrap_or("")
                                    .chars().take(30).collect::<String>()),
                            )
                            .size(LabelSize::Small)
                            .color(Color::Accent),
                        )
                    })
                    .child(gpui::div().flex_1())
                    .child(
                        Button::new("refresh-plans", "↻")
                            .style(ButtonStyle::Transparent)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh(cx);
                            })),
                    ),
            )
            .child(content)
    }
}

impl Panel for PlansPanel {
    fn persistent_name() -> &'static str {
        "PrismPlansPanel"
    }

    fn panel_key() -> &'static str {
        PLANS_PANEL_KEY
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _window: &mut Window, cx: &mut Context<Self>) {
        self.position = position;
        cx.notify();
    }

    fn size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        self.width.unwrap_or(px(300.0))
    }

    fn set_size(&mut self, size: Option<gpui::Pixels>, _window: &mut Window, cx: &mut Context<Self>) {
        self.width = size;
        cx.notify();
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<ui::IconName> {
        Some(ui::IconName::Ai)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Plans")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(TogglePlansPanel)
    }

    fn activation_priority(&self) -> u32 {
        9
    }
}
