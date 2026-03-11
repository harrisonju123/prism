use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Window, actions,
};
use ui::{Color, Label, LabelSize, h_flex, prelude::*, v_flex};
use workspace::Workspace;
use workspace::item::{Item, ItemEvent};

use crate::hq_state::HqState;
use uglyhat::model::{Plan, PlanStatus, WorkPackage, WorkPackageStatus};

actions!(prism_hq, [OpenPlanView]);

pub struct PlanViewItem {
    focus_handle: FocusHandle,
    hq_state: Entity<HqState>,
    _hq_subscription: Subscription,
    /// If set, show only this plan's WPs; otherwise show all active plans.
    pub plan_id: Option<uuid::Uuid>,
}

impl PlanViewItem {
    pub fn new(
        hq_state: Entity<HqState>,
        plan_id: Option<uuid::Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();
        let subscription = cx.observe(&hq_state, |_, _, cx| cx.notify());
        Self {
            focus_handle,
            hq_state,
            _hq_subscription: subscription,
            plan_id,
        }
    }
}

impl EventEmitter<ItemEvent> for PlanViewItem {}

impl Focusable for PlanViewItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for PlanViewItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Plans".into()
    }

    fn to_item_events(event: &ItemEvent, f: &mut dyn FnMut(ItemEvent)) {
        f(event.clone());
    }
}

impl Render for PlanViewItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (plans, all_wps) = {
            let state = self.hq_state.read(cx);
            (state.plans.clone(), state.work_packages.clone())
        };

        let plans_to_show: Vec<Plan> = if let Some(pid) = self.plan_id {
            plans.into_iter().filter(|p| p.id == pid).collect()
        } else {
            plans
        };

        if plans_to_show.is_empty() {
            return v_flex()
                .size_full()
                .p_4()
                .items_center()
                .justify_center()
                .child(
                    Label::new("No active plans. Use '+ New Task' to create one.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                );
        }

        // Pre-extract colors to avoid borrowing cx inside closures.
        let surface_bg = cx.theme().colors().surface_background;
        let border_color = cx.theme().colors().border;
        let editor_bg = cx.theme().colors().editor_background;

        let plan_count = plans_to_show.len();

        // Build plan section elements outside of layout closures.
        let mut section_elements: Vec<gpui::AnyElement> = Vec::new();
        for plan in &plans_to_show {
            let plan_wps: Vec<WorkPackage> = all_wps
                .iter()
                .filter(|wp| wp.plan_id == Some(plan.id))
                .cloned()
                .collect();

            let done = plan_wps
                .iter()
                .filter(|wp| wp.status == WorkPackageStatus::Done)
                .count();
            let total = plan_wps.len();

            let plan_status_color = match plan.status {
                PlanStatus::Active => Color::Accent,
                PlanStatus::Approved => Color::Warning,
                PlanStatus::Completed => Color::Success,
                PlanStatus::Cancelled => Color::Disabled,
                PlanStatus::Draft => Color::Muted,
            };

            let intent_preview = if plan.intent.len() > 70 {
                format!("{}…", &plan.intent[..70])
            } else {
                plan.intent.clone()
            };

            let mut wp_elements: Vec<gpui::AnyElement> = Vec::new();
            for (ix, wp) in plan_wps.iter().enumerate() {
                let status_color = match wp.status {
                    WorkPackageStatus::Done => Color::Success,
                    WorkPackageStatus::InProgress => Color::Accent,
                    WorkPackageStatus::Review => Color::Warning,
                    WorkPackageStatus::Ready => Color::Default,
                    WorkPackageStatus::Planned | WorkPackageStatus::Draft => Color::Muted,
                    WorkPackageStatus::Cancelled => Color::Disabled,
                };
                let status_label: SharedString = match wp.status {
                    WorkPackageStatus::Draft => "draft".into(),
                    WorkPackageStatus::Planned => "planned".into(),
                    WorkPackageStatus::Ready => "ready".into(),
                    WorkPackageStatus::InProgress => "in progress".into(),
                    WorkPackageStatus::Review => "review".into(),
                    WorkPackageStatus::Done => "done".into(),
                    WorkPackageStatus::Cancelled => "cancelled".into(),
                };

                let intent = wp.intent.clone();
                let agent = wp.assigned_agent.clone();
                let ordinal = wp.ordinal;

                let row = h_flex()
                    .id(("wp-row", ix + plan.id.as_u128() as usize))
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(border_color)
                    .child(
                        Label::new(format!("#{}", ordinal + 1))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(div().flex_1().child(Label::new(intent).size(LabelSize::Small)))
                    .child(
                        Label::new(status_label)
                            .size(LabelSize::XSmall)
                            .color(status_color),
                    )
                    .when_some(agent, |this, a| {
                        this.child(Label::new(a).size(LabelSize::XSmall).color(Color::Accent))
                    });

                wp_elements.push(row.into_any_element());
            }

            let section = v_flex()
                .w_full()
                .mb_3()
                .child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .bg(surface_bg)
                        .border_b_1()
                        .border_color(border_color)
                        .gap_2()
                        .child(
                            Label::new(plan.status.to_string())
                                .size(LabelSize::XSmall)
                                .color(plan_status_color),
                        )
                        .child(div().flex_1().child(Label::new(intent_preview).size(LabelSize::Small)))
                        .child(
                            Label::new(format!("{done}/{total}"))
                                .size(LabelSize::XSmall)
                                .color(if done == total && total > 0 {
                                    Color::Success
                                } else {
                                    Color::Muted
                                }),
                        ),
                )
                .children(wp_elements);

            section_elements.push(section.into_any_element());
        }

        v_flex()
            .size_full()
            .bg(editor_bg)
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(border_color)
                    .child(Label::new("Plans").size(LabelSize::Small))
                    .flex_1()
                    .child(
                        Label::new(format!("{} active", plan_count))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                v_flex()
                    .id("plans-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_2()
                    .children(section_elements),
            )
    }
}

pub fn open_plan_view(
    workspace: &mut Workspace,
    hq_state: Entity<HqState>,
    plan_id: Option<uuid::Uuid>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let item = cx.new(|cx| PlanViewItem::new(hq_state, plan_id, window, cx));
    workspace.add_item_to_center(Box::new(item), window, cx);
}
