use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, WeakEntity, Window, actions,
};
use ui::{Color, Icon, IconName, Label, LabelSize, h_flex, prelude::*, v_flex};
use workspace::{
    Workspace,
    item::{Item, ItemEvent},
};

use crate::agent_view::open_agent_view;
use crate::hq_state::HqState;
use crate::plan_dispatch::PlanDispatchModal;
use crate::thread_view::open_thread_view;

actions!(prism_hq, [OpenCommandCenter]);

pub struct CommandCenterItem {
    focus_handle: FocusHandle,
    hq_state: Entity<HqState>,
    workspace: Option<WeakEntity<Workspace>>,
    _hq_subscription: Subscription,
}

impl CommandCenterItem {
    pub fn new(
        hq_state: Entity<HqState>,
        workspace: Option<WeakEntity<Workspace>>,
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
            workspace,
            _hq_subscription: subscription,
        }
    }

    fn render_agents_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.hq_state.read(cx);
        let agents = state
            .overview
            .as_ref()
            .map(|o| o.active_agents.clone())
            .unwrap_or_default();
        let is_loading = state.is_loading;
        let has_overview = state.overview.is_some();

        v_flex()
            .w_1_4()
            .h_full()
            .border_r_1()
            .border_color(cx.theme().colors().border)
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .h(px(32.))
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Label::new("AGENTS")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                v_flex()
                    .id("agents-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .when(agents.is_empty() && !is_loading, |this| {
                        this.child(
                            Label::new("No active agents")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .when(is_loading && !has_overview, |this| {
                        this.child(
                            Label::new("Loading…")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .children(agents.into_iter().enumerate().map(|(ix, agent)| {
                        let state_color = match agent.state {
                            uglyhat::model::AgentState::Working => Color::Accent,
                            uglyhat::model::AgentState::Idle => Color::Success,
                            uglyhat::model::AgentState::Blocked => Color::Warning,
                            uglyhat::model::AgentState::Dead => Color::Muted,
                        };
                        let state_label = agent.state.to_string();
                        let agent_name = agent.name.clone();
                        let agent_name_for_click = agent.name.clone();
                        let thread_name = agent.current_thread.clone();
                        let ws = self.workspace.clone();
                        v_flex()
                            .id(("agent-card", ix))
                            .w_full()
                            .p_1()
                            .gap_0p5()
                            .rounded_sm()
                            .bg(cx.theme().colors().element_background)
                            .cursor_pointer()
                            .hover(|s| s.bg(cx.theme().colors().element_hover))
                            .on_click(cx.listener(move |_, _, window, cx| {
                                if let Some(ws_ref) = ws.as_ref().and_then(|w| w.upgrade()) {
                                    ws_ref.update(cx, |workspace, cx| {
                                        open_agent_view(
                                            workspace,
                                            agent_name_for_click.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(Label::new(agent_name).size(LabelSize::Small))
                                    .child(
                                        Label::new(state_label)
                                            .size(LabelSize::XSmall)
                                            .color(state_color),
                                    ),
                            )
                            .when_some(thread_name, |this, thread| {
                                this.child(
                                    Label::new(thread)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            })
                    })),
            )
    }

    fn render_activity_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.hq_state.read(cx);
        let activity = state.activity.clone();

        v_flex()
            .flex_1()
            .h_full()
            .border_r_1()
            .border_color(cx.theme().colors().border)
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .h(px(32.))
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Label::new("ACTIVITY FEED")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                v_flex()
                    .id("activity-feed")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .py_1()
                    .gap_0p5()
                    .when(activity.is_empty(), |this| {
                        this.child(
                            Label::new("No recent activity")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .children(activity.into_iter().enumerate().map(|(ix, entry)| {
                        let actor = entry.actor.clone();
                        let summary = if !entry.summary.is_empty() {
                            entry.summary.clone()
                        } else {
                            format!("{} {}", entry.action, entry.entity_type)
                        };
                        let is_thread = entry.entity_type == "thread";
                        let ws = self.workspace.clone();
                        let entity_name = entry.entity_type.clone();
                        h_flex()
                            .id(("activity-entry", ix))
                            .w_full()
                            .gap_1()
                            .rounded_sm()
                            .when(is_thread, |this| {
                                this.cursor_pointer()
                                    .hover(|s| s.bg(cx.theme().colors().element_hover))
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        if let Some(ws_ref) = ws.as_ref().and_then(|w| w.upgrade())
                                        {
                                            let name = entity_name.clone();
                                            ws_ref.update(cx, |workspace, cx| {
                                                open_thread_view(workspace, name, window, cx);
                                            });
                                        }
                                    }))
                            })
                            .when(!actor.is_empty(), |this| {
                                this.child(
                                    Label::new(actor)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Accent),
                                )
                            })
                            .child(Label::new(summary).size(LabelSize::XSmall))
                    })),
            )
    }

    fn render_summary_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.hq_state.read(cx);
        let workspace_name = state
            .overview
            .as_ref()
            .map(|o| o.workspace.name.clone())
            .unwrap_or_else(|| "—".to_string());
        let thread_count = state
            .overview
            .as_ref()
            .map(|o| o.active_threads.len())
            .unwrap_or(0);
        let agent_count = state
            .overview
            .as_ref()
            .map(|o| o.active_agents.len())
            .unwrap_or(0);
        let memory_count = state
            .overview
            .as_ref()
            .map(|o| o.recent_memories.len())
            .unwrap_or(0);
        let error = state.error.clone();

        v_flex()
            .w_1_4()
            .h_full()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .h(px(32.))
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Label::new("WORKSPACE")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .child(Label::new(workspace_name).size(LabelSize::Small))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Label::new("Threads")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(thread_count.to_string())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Accent),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Label::new("Agents")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(agent_count.to_string())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Accent),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Label::new("Memories")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(memory_count.to_string())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Accent),
                            ),
                    )
                    .when_some(error, |this, err| {
                        this.child(Label::new(err).size(LabelSize::XSmall).color(Color::Error))
                    }),
            )
    }
}

impl EventEmitter<ItemEvent> for CommandCenterItem {}

impl Focusable for CommandCenterItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for CommandCenterItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Command Center".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::AiZed))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

impl Render for CommandCenterItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.clone();

        v_flex()
            .key_context("CommandCenter")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            // Dispatch bar at the top
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .gap_2()
                    .child(
                        div()
                            .id("dispatch-bar-input")
                            .flex_1()
                            .px_2()
                            .py_1()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .rounded_md()
                            .bg(cx.theme().colors().surface_background)
                            .cursor_pointer()
                            .on_click(cx.listener(move |_, _, window, cx| {
                                if let Some(ws_ref) = ws.as_ref().and_then(|w| w.upgrade()) {
                                    ws_ref.update(cx, |workspace, cx| {
                                        PlanDispatchModal::open(workspace, window, cx);
                                    });
                                }
                            }))
                            .child(
                                Label::new("What should an agent work on?")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    ),
            )
            // Main columns
            .child(
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_agents_column(cx))
                    .child(self.render_activity_column(cx))
                    .child(self.render_summary_column(cx)),
            )
    }
}

/// Open or activate the Command Center in the active workspace.
pub fn open_command_center(
    workspace: &mut workspace::Workspace,
    hq_state: Entity<HqState>,
    window: &mut Window,
    cx: &mut Context<workspace::Workspace>,
) {
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<CommandCenterItem>());

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let weak_workspace = cx.weak_entity();
        let item = cx.new(|cx: &mut Context<CommandCenterItem>| {
            CommandCenterItem::new(hq_state, Some(weak_workspace), window, cx)
        });
        workspace.add_item_to_center(Box::new(item), window, cx);
    }
}
