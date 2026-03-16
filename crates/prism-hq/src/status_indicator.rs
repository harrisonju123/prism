use std::sync::Arc;

use gpui::{Action, Context, Render};
use prism_context::model::AgentState;
use ui::{ButtonLike, Color, ContextMenu, Label, LabelSize, PopoverMenu, PopoverMenuHandle, prelude::*};
use workspace::StatusItemView;

use crate::dispatch::DispatchTask;
use crate::hq_state::HqState;
use crate::plan_dispatch::DispatchPlan;

pub struct PrismStatusIndicator {
    popover_menu_handle: PopoverMenuHandle<ContextMenu>,
    _hq_subscription: Option<gpui::Subscription>,
    /// (name, state, current_thread) — Arc for cheap clones in render
    agent_summaries: Arc<Vec<(String, AgentState, Option<String>)>>,
    actionable_count: usize,
    /// Cached label — recomputed only when state changes
    label: String,
    /// Cached dot color — recomputed only when state changes
    dot_color: Color,
}

fn compute_label(agent_count: usize, actionable_count: usize) -> String {
    if agent_count == 0 {
        "P ● idle".to_string()
    } else {
        let mut s = format!(
            "P ● {} agent{}",
            agent_count,
            if agent_count == 1 { "" } else { "s" }
        );
        if actionable_count > 0 {
            s.push_str(&format!(
                " · {} review",
                actionable_count
            ));
        }
        s
    }
}

fn compute_dot_color(
    summaries: &[(String, AgentState, Option<String>)],
    actionable_count: usize,
) -> Color {
    if summaries.iter().any(|(_, state, _)| *state == AgentState::Blocked) {
        Color::Error
    } else if actionable_count > 0 {
        Color::Warning
    } else {
        Color::Success
    }
}

fn status_row(
    text: String,
) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyElement + 'static {
    move |_, _| {
        Label::new(text.clone())
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element()
    }
}

impl PrismStatusIndicator {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let hq_subscription = HqState::global(cx).map(|hq_entity| {
            cx.observe(&hq_entity, |this, hq_entity, cx| {
                let hq = hq_entity.read(cx);
                let new_actionable = hq.inbox_entries.iter().filter(|e| !e.read).count();

                // Skip notify if nothing relevant changed.
                let unchanged = hq.agents.len() == this.agent_summaries.len()
                    && new_actionable == this.actionable_count
                    && hq.agents
                        .iter()
                        .zip(this.agent_summaries.iter())
                        .all(|(a, (name, state, thread))| {
                            a.name == *name
                                && a.state == *state
                                && a.current_thread == *thread
                        });
                if unchanged {
                    return;
                }

                let summaries = Arc::new(
                    hq.agents
                        .iter()
                        .map(|a| (a.name.clone(), a.state.clone(), a.current_thread.clone()))
                        .collect::<Vec<_>>(),
                );
                this.label = compute_label(summaries.len(), new_actionable);
                this.dot_color = compute_dot_color(&summaries, new_actionable);
                this.agent_summaries = summaries;
                this.actionable_count = new_actionable;
                cx.notify();
            })
        });

        Self {
            popover_menu_handle: PopoverMenuHandle::default(),
            _hq_subscription: hq_subscription,
            agent_summaries: Arc::new(Vec::new()),
            actionable_count: 0,
            label: "P ● idle".to_string(),
            dot_color: Color::Success,
        }
    }
}

impl Render for PrismStatusIndicator {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut Context<Self>,
    ) -> impl gpui::IntoElement {
        let label = self.label.clone();
        let dot_color = self.dot_color;
        let agent_summaries = self.agent_summaries.clone(); // cheap Arc clone
        let actionable_count = self.actionable_count;

        gpui::div().child(
            PopoverMenu::new("prism-status-popover")
                .anchor(gpui::Corner::BottomLeft)
                .menu(move |window, cx| {
                    let agent_summaries = agent_summaries.clone(); // cheap Arc clone
                    Some(ContextMenu::build(window, cx, move |menu, _, _| {
                        let mut menu = menu.header("Agents");

                        if agent_summaries.is_empty() {
                            menu = menu.custom_row(status_row("No active agents".to_string()));
                        } else {
                            for (name, state, thread) in agent_summaries.as_ref() {
                                let row_label = if let Some(t) = thread {
                                    format!("{name}  {state}  [{t}]")
                                } else {
                                    format!("{name}  {state}")
                                };
                                menu = menu.custom_row(status_row(row_label));
                            }
                        }

                        if actionable_count > 0 {
                            menu = menu.separator().header(format!(
                                "{actionable_count} item{} need review",
                                if actionable_count == 1 { "" } else { "s" }
                            ));
                        }

                        menu.separator()
                            .action("Dispatch Task...", DispatchTask.boxed_clone())
                            .action("Dispatch Plan...", DispatchPlan.boxed_clone())
                    }))
                })
                .trigger(
                    ButtonLike::new("prism-status-trigger").child(
                        Label::new(label).size(LabelSize::Small).color(dot_color),
                    ),
                )
                .with_handle(self.popover_menu_handle.clone()),
        )
    }
}

impl StatusItemView for PrismStatusIndicator {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn workspace::item::ItemHandle>,
        _window: &mut gpui::Window,
        _cx: &mut Context<Self>,
    ) {
    }
}
