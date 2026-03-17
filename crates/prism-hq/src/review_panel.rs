use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, Render, Task, WeakEntity,
    Window, actions, px,
};
use prism_context::model::{
    AssumptionStatus, ChangeSet, Plan, ValidationStatus, WorkPackage, WorkPackageStatus,
};
use ui::{Button, ButtonStyle, Color, Icon, IconName, Label, LabelSize, prelude::*, v_flex, h_flex};
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::context_service::ContextService;

actions!(prism_hq, [ToggleReviewPanel]);

const REVIEW_PANEL_KEY: &str = "prism_review_panel";

pub struct ReviewPanel {
    focus_handle: FocusHandle,
    position: DockPosition,
    width: Option<gpui::Pixels>,
    // Data
    plan: Option<Plan>,
    work_packages: Vec<WorkPackage>,
    change_sets: Vec<ChangeSet>,
    is_loading: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
}

impl EventEmitter<PanelEvent> for ReviewPanel {}

impl ReviewPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let auto_refresh = cx.spawn(async move |this: WeakEntity<ReviewPanel>, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(10))
                    .await;
                this.update(cx, |panel, cx| panel.refresh(cx)).ok();
            }
        });

        let mut panel = Self {
            focus_handle,
            position: DockPosition::Right,
            width: None,
            plan: None,
            work_packages: Vec::new(),
            change_sets: Vec::new(),
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

            type Result3 = anyhow::Result<(Option<Plan>, Vec<WorkPackage>, Vec<ChangeSet>)>;
            let result: Result3 = cx
                .background_spawn(async move {
                    let handle = handle
                        .ok_or_else(|| anyhow::anyhow!("context service not available"))?;
                    let plan = handle.get_active_plan()?;
                    let (wps, change_sets) = if let Some(ref p) = plan {
                        let wps = handle.list_work_packages(Some(p.id), None)?;
                        let sets = handle.list_change_sets(Some(p.id), None)?;
                        (wps, sets)
                    } else {
                        (Vec::new(), Vec::new())
                    };
                    anyhow::Ok((plan, wps, change_sets))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok((plan, wps, sets)) => {
                        this.plan = plan;
                        this.work_packages = wps;
                        this.change_sets = sets;
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

    fn validation_badge_color(status: &ValidationStatus) -> Color {
        match status {
            ValidationStatus::Passing => Color::Success,
            ValidationStatus::Failing => Color::Error,
            ValidationStatus::Pending => Color::Muted,
            ValidationStatus::Skipped => Color::Muted,
        }
    }
}

impl Focusable for ReviewPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ReviewPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self.plan.clone();
        let work_packages = self.work_packages.clone();
        let change_sets = self.change_sets.clone();
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

        if let Some(ref plan) = plan {
            // Plan header
            content = content.child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(plan.intent.chars().take(80).collect::<String>())
                            .size(LabelSize::Small)
                            .color(Color::Default),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Label::new(plan.current_phase.to_string())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Accent),
                            )
                            .child(
                                Label::new(plan.autonomy_level.to_string())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    ),
            );

            // Assumptions section
            if !plan.assumptions.is_empty() {
                let mut assump_section = v_flex().gap_1().child(
                    Label::new("Assumptions")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                );
                for assumption in plan.assumptions.iter().take(8) {
                    let (icon, color) = match assumption.status {
                        AssumptionStatus::Confirmed => (IconName::Check, Color::Success),
                        AssumptionStatus::Rejected => (IconName::Close, Color::Error),
                        AssumptionStatus::Unverified => (IconName::CircleHelp, Color::Warning),
                    };
                    assump_section = assump_section.child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(icon).size(ui::IconSize::XSmall).color(color))
                            .child(
                                Label::new(assumption.text.chars().take(60).collect::<String>())
                                    .size(LabelSize::XSmall),
                            ),
                    );
                }
                content = content.child(assump_section);
            }

            // Work packages with validation status
            if !work_packages.is_empty() {
                let done = work_packages
                    .iter()
                    .filter(|w| w.status == WorkPackageStatus::Done)
                    .count();
                let mut wp_section = v_flex().gap_1().child(
                    h_flex()
                        .gap_2()
                        .child(
                            Label::new("Work Packages")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new(format!("{}/{}", done, work_packages.len()))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                );
                for wp in work_packages.iter().take(10) {
                    let status_color = match wp.status {
                        WorkPackageStatus::Done => Color::Success,
                        WorkPackageStatus::InProgress => Color::Accent,
                        WorkPackageStatus::Cancelled => Color::Disabled,
                        _ => Color::Muted,
                    };
                    let val_color = Self::validation_badge_color(&wp.validation_status);
                    wp_section = wp_section.child(
                        h_flex()
                            .gap_1()
                            .child(
                                Label::new(wp.status.to_string())
                                    .size(LabelSize::XSmall)
                                    .color(status_color),
                            )
                            .child(
                                Label::new(wp.intent.chars().take(50).collect::<String>())
                                    .size(LabelSize::XSmall),
                            )
                            .child(
                                Label::new(wp.validation_status.to_string())
                                    .size(LabelSize::XSmall)
                                    .color(val_color),
                            ),
                    );
                }
                content = content.child(wp_section);
            }

            // Change sets section — deduplicate by file path, keeping latest entry per file
            if !change_sets.is_empty() {
                let mut seen = std::collections::HashSet::new();
                let unique_sets: Vec<_> = change_sets
                    .iter()
                    .rev()
                    .filter(|cs| seen.insert(cs.file_path.clone()))
                    .collect();
                let mut cs_section = v_flex().gap_1().child(
                    h_flex()
                        .gap_2()
                        .child(
                            Label::new("Changes")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new(format!("{} files", unique_sets.len()))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                );
                for cs in unique_sets.iter().take(12) {
                    let type_color = match cs.change_type {
                        prism_context::model::ChangeType::Added => Color::Success,
                        prism_context::model::ChangeType::Deleted => Color::Error,
                        _ => Color::Warning,
                    };
                    let type_label = match cs.change_type {
                        prism_context::model::ChangeType::Added => "A",
                        prism_context::model::ChangeType::Modified => "M",
                        prism_context::model::ChangeType::Deleted => "D",
                        prism_context::model::ChangeType::Renamed => "R",
                    };
                    // Show basename only
                    let file_name = std::path::Path::new(&cs.file_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&cs.file_path)
                        .to_string();
                    cs_section = cs_section.child(
                        h_flex()
                            .gap_1()
                            .child(
                                Label::new(type_label)
                                    .size(LabelSize::XSmall)
                                    .color(type_color),
                            )
                            .child(Label::new(file_name).size(LabelSize::XSmall)),
                    );
                }
                content = content.child(cs_section);
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
                    .child(Label::new("Review").size(LabelSize::Small).color(Color::Muted))
                    .child(gpui::div().flex_1())
                    .child(
                        Button::new("refresh-review", "↻")
                            .style(ButtonStyle::Transparent)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh(cx);
                            })),
                    ),
            )
            .child(content)
    }
}

impl Panel for ReviewPanel {
    fn persistent_name() -> &'static str {
        "PrismReviewPanel"
    }

    fn panel_key() -> &'static str {
        REVIEW_PANEL_KEY
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.position = position;
        cx.notify();
    }

    fn size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        self.width.unwrap_or(px(300.0))
    }

    fn set_size(
        &mut self,
        size: Option<gpui::Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.width = size;
        cx.notify();
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Ai)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Prism Review")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleReviewPanel)
    }

    fn activation_priority(&self) -> u32 {
        9
    }
}
