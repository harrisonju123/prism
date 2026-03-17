use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, Render, Task, WeakEntity,
    Window, actions, px,
};
use prism_context::model::{
    AssumptionStatus, BlockerStatus, Plan, WorkPackage, WorkPackageStatus,
};
use ui::{Button, ButtonStyle, Color, Label, LabelSize, prelude::*, v_flex, h_flex};
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::context_service::ContextService;
use crate::hq_state::HqState;

actions!(prism_hq, [ToggleMissionPanel]);

const MISSION_PANEL_KEY: &str = "prism_mission_panel";

pub struct MissionPanel {
    focus_handle: FocusHandle,
    _hq_subscription: Option<gpui::Subscription>,
    position: DockPosition,
    width: Option<gpui::Pixels>,
    // Data
    plan: Option<Plan>,
    work_packages: Vec<WorkPackage>,
    is_loading: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
}

impl EventEmitter<PanelEvent> for MissionPanel {}

impl MissionPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let auto_refresh = cx.spawn(async move |this: WeakEntity<MissionPanel>, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                this.update(cx, |panel, cx| panel.refresh(cx)).ok();
            }
        });

        let hq_subscription = HqState::global(cx).map(|hq_entity| {
            cx.observe(&hq_entity, |this, hq, cx| {
                if let Some(plan) = hq.read(cx).active_plan() {
                    this.plan = Some(plan.clone());
                    cx.notify();
                }
            })
        });

        let mut panel = Self {
            focus_handle,
            _hq_subscription: hq_subscription,
            position: DockPosition::Right,
            width: None,
            plan: None,
            work_packages: Vec::new(),
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

            let result: anyhow::Result<(Option<Plan>, Vec<WorkPackage>)> = cx
                .background_spawn(async move {
                    let handle = handle
                        .ok_or_else(|| anyhow::anyhow!("context service not available"))?;
                    let plan = handle.get_active_plan()?;
                    let wps = if let Some(ref p) = plan {
                        handle.list_work_packages(Some(p.id), None)?
                    } else {
                        vec![]
                    };
                    anyhow::Ok((plan, wps))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok((plan, wps)) => {
                        this.plan = plan;
                        this.work_packages = wps;
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

impl Focusable for MissionPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MissionPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self.plan.clone();
        let work_packages = self.work_packages.clone();
        let is_loading = self.is_loading;
        let error = self.error.clone();

        let mut content = v_flex().flex_1().overflow_hidden().p_2().gap_3();

        if is_loading && plan.is_none() {
            content = content.child(
                Label::new("Loading…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        }

        if let Some(err) = error {
            content = content.child(
                Label::new(format!("Error: {err}"))
                    .size(LabelSize::Small)
                    .color(Color::Error),
            );
        }

        if let Some(plan) = plan {
            // Objective
            content = content.child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Objective").size(LabelSize::Small).color(Color::Muted))
                    .child(Label::new(plan.intent.clone()).size(LabelSize::Small)),
            );

            // Phase timeline
            let phases = prism_context::model::MissionPhase::all();
            let current_phase_str = plan.current_phase.to_string();
            content = content.child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Phase").size(LabelSize::Small).color(Color::Muted))
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(phases.iter().map(|p| {
                                let is_current = *p == current_phase_str.as_str();
                                Label::new(*p)
                                    .size(LabelSize::Small)
                                    .color(if is_current { Color::Accent } else { Color::Muted })
                            })),
                    ),
            );

            // Autonomy
            content = content.child(
                h_flex()
                    .gap_1()
                    .child(Label::new("Autonomy").size(LabelSize::Small).color(Color::Muted))
                    .child(Label::new(plan.autonomy_level.to_string()).size(LabelSize::Small)),
            );

            // Assumptions
            if !plan.assumptions.is_empty() {
                content = content.child(
                    v_flex()
                        .gap_1()
                        .child(
                            Label::new(format!("Assumptions ({})", plan.assumptions.len()))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .children(plan.assumptions.iter().map(|a| {
                            let status_color = match a.status {
                                AssumptionStatus::Confirmed => Color::Success,
                                AssumptionStatus::Rejected => Color::Error,
                                AssumptionStatus::Unverified => Color::Warning,
                            };
                            h_flex()
                                .gap_1()
                                .child(
                                    Label::new(format!("[{}]", a.status))
                                        .size(LabelSize::Small)
                                        .color(status_color),
                                )
                                .child(
                                    Label::new(a.text.chars().take(60).collect::<String>())
                                        .size(LabelSize::Small),
                                )
                        })),
                );
            }

            // Blockers
            let open_blockers: Vec<_> = plan.blockers.iter().filter(|b| b.status == BlockerStatus::Open).collect();
            if !open_blockers.is_empty() {
                content = content.child(
                    v_flex()
                        .gap_1()
                        .child(
                            Label::new(format!("Blockers ({} open)", open_blockers.len()))
                                .size(LabelSize::Small)
                                .color(Color::Error),
                        )
                        .children(open_blockers.into_iter().map(|b| {
                            Label::new(b.text.chars().take(60).collect::<String>())
                                .size(LabelSize::Small)
                                .color(Color::Error)
                        })),
                );
            }

            // Work packages
            if !work_packages.is_empty() {
                let done = work_packages.iter().filter(|w| w.status == WorkPackageStatus::Done).count();
                let total = work_packages.len();
                content = content.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Label::new("Work Packages")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(
                                    Label::new(format!("{}/{}", done, total))
                                        .size(LabelSize::Small),
                                ),
                        )
                        .children(work_packages.iter().map(|wp| {
                            let status_color = match wp.status {
                                WorkPackageStatus::Done => Color::Success,
                                WorkPackageStatus::InProgress => Color::Accent,
                                WorkPackageStatus::Cancelled => Color::Muted,
                                _ => Color::Default,
                            };
                            h_flex()
                                .gap_1()
                                .child(
                                    Label::new(format!("[{}]", wp.status))
                                        .size(LabelSize::Small)
                                        .color(status_color),
                                )
                                .child(
                                    Label::new(wp.intent.chars().take(50).collect::<String>())
                                        .size(LabelSize::Small),
                                )
                        })),
                );
            }

            // Files touched
            if !plan.files_touched.is_empty() {
                content = content.child(
                    v_flex()
                        .gap_1()
                        .child(
                            Label::new(format!("Files touched ({})", plan.files_touched.len()))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .children(plan.files_touched.iter().take(10).map(|f| {
                            Label::new(
                                std::path::Path::new(f)
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(f.as_str())
                                    .to_string()
                            )
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                        })),
                );
            }
        } else {
            content = content.child(
                Label::new(if is_loading { "Loading…" } else { "No active mission" })
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
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
                    .child(Label::new("Mission").size(LabelSize::Small).color(Color::Muted))
                    .child(gpui::div().flex_1())
                    .child(
                        Button::new("refresh-mission", "↻")
                            .style(ButtonStyle::Transparent)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh(cx);
                            })),
                    ),
            )
            .child(content)
    }
}

impl Panel for MissionPanel {
    fn persistent_name() -> &'static str {
        "PrismMissionPanel"
    }

    fn panel_key() -> &'static str {
        MISSION_PANEL_KEY
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
        self.width.unwrap_or(px(280.0))
    }

    fn set_size(&mut self, size: Option<gpui::Pixels>, _window: &mut Window, cx: &mut Context<Self>) {
        self.width = size;
        cx.notify();
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<ui::IconName> {
        Some(ui::IconName::Ai)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Mission Control")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleMissionPanel)
    }

    fn activation_priority(&self) -> u32 {
        9
    }
}
