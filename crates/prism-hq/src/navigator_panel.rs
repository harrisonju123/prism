use db::kvp::KEY_VALUE_STORE;
use gpui::{
    Action, App, AppContext as _, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle,
    Focusable, IntoElement, ParentElement, Pixels, Render, Styled, Task, WeakEntity, Window,
    actions, px,
};
use serde::{Deserialize, Serialize};
use ui::IconName;
use ui::{Color, Label, LabelSize, Tooltip, h_flex, prelude::*, v_flex};
use util::{ResultExt, TryFutureExt};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use crate::agent_view::open_agent_view;
use crate::hq_state::HqState;
use crate::inbox_item::open_inbox;
use crate::plan_dispatch::PlanDispatchModal;
use crate::plan_view::open_plan_view;
use crate::running_agents::RunningAgents;
use crate::thread_view::open_thread_view;

const PANEL_KEY: &str = "HqNavigatorPanel";

actions!(
    prism_hq,
    [
        /// Toggles the Agent HQ navigator panel.
        ToggleNavigator,
        /// Moves focus to the Agent HQ navigator panel.
        FocusNavigator,
    ]
);

pub struct NavigatorPanel {
    focus_handle: FocusHandle,
    width: Option<Pixels>,
    active: bool,
    hq_state: Option<Entity<HqState>>,
    workspace: Option<WeakEntity<Workspace>>,
    pending_serialization: Task<Option<()>>,
}

#[derive(Serialize, Deserialize)]
struct SerializedNavigatorPanel {
    width: Option<Pixels>,
}

#[derive(Debug)]
pub enum Event {
    DockPositionChanged,
    Focus,
    Dismissed,
}

impl NavigatorPanel {
    pub fn new(
        _workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let weak_workspace = cx.weak_entity();
        cx.new(|cx: &mut Context<NavigatorPanel>| {
            let hq_state = HqState::global(cx);
            NavigatorPanel {
                focus_handle: cx.focus_handle(),
                width: None,
                active: false,
                hq_state,
                workspace: Some(weak_workspace),
                pending_serialization: Task::ready(None),
            }
        })
    }

    pub fn load(
        workspace: WeakEntity<Workspace>,
        cx: AsyncWindowContext,
    ) -> Task<anyhow::Result<Entity<Self>>> {
        cx.spawn(async move |cx| {
            let serialized = cx
                .background_spawn(async move { KEY_VALUE_STORE.read_kvp(PANEL_KEY) })
                .await
                .log_err()
                .flatten()
                .and_then(|s| serde_json::from_str::<SerializedNavigatorPanel>(&s).log_err());

            workspace.update_in(cx, |workspace, window, cx| {
                let panel = Self::new(workspace, window, cx);
                if let Some(serialized) = serialized {
                    panel.update(cx, |panel, cx| {
                        panel.width = serialized.width.map(|w| w.round());
                        cx.notify();
                    });
                }
                panel
            })
        })
    }

    fn serialize(&mut self, cx: &mut Context<Self>) {
        let width = self.width;
        self.pending_serialization = cx.background_spawn(
            async move {
                KEY_VALUE_STORE
                    .write_kvp(
                        PANEL_KEY.into(),
                        serde_json::to_string(&SerializedNavigatorPanel { width })?,
                    )
                    .await?;
                anyhow::Ok(())
            }
            .log_err(),
        );
    }
}

impl EventEmitter<PanelEvent> for NavigatorPanel {}
impl EventEmitter<Event> for NavigatorPanel {}

impl Focusable for NavigatorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for NavigatorPanel {
    fn persistent_name() -> &'static str {
        "HqNavigatorPanel"
    }

    fn panel_key() -> &'static str {
        PANEL_KEY
    }

    fn position(&self, _: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left)
    }

    fn set_position(&mut self, _position: DockPosition, _: &mut Window, _cx: &mut Context<Self>) {}

    fn size(&self, _: &Window, _cx: &App) -> Pixels {
        self.width.unwrap_or(px(200.))
    }

    fn set_size(&mut self, size: Option<Pixels>, _: &mut Window, cx: &mut Context<Self>) {
        self.width = size;
        self.serialize(cx);
        cx.notify();
    }

    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.active = active;
        cx.notify();
    }

    fn icon(&self, _: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::AiZed)
    }

    fn icon_tooltip(&self, _: &Window, _cx: &App) -> Option<&'static str> {
        Some("Agent Navigator")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        ToggleNavigator.boxed_clone()
    }

    fn activation_priority(&self) -> u32 {
        12
    }
}

impl Render for NavigatorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (threads, agents, handoffs, unread_inbox, plans) = self
            .hq_state
            .as_ref()
            .map(|s| {
                let state = s.read(cx);
                let unread_inbox = state.unread_inbox_count;
                let threads: Vec<(String, usize)> = state
                    .threads
                    .iter()
                    .map(|t| {
                        let assigned = state
                            .handoffs
                            .iter()
                            .filter(|h| h.thread_id == Some(t.id))
                            .count();
                        (t.name.clone(), assigned)
                    })
                    .collect();
                let agents: Vec<(String, uglyhat::model::AgentState, Option<String>)> = state
                    .overview
                    .as_ref()
                    .map(|o| {
                        o.active_agents
                            .iter()
                            .map(|a| (a.name.clone(), a.state.clone(), a.current_thread.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let pending_handoffs: Vec<String> = state
                    .handoffs
                    .iter()
                    .filter(|h| {
                        matches!(
                            h.status,
                            uglyhat::model::HandoffStatus::Pending
                                | uglyhat::model::HandoffStatus::Running
                        )
                    })
                    .map(|h| {
                        if h.task.len() > 40 {
                            format!("{}…", &h.task[..40])
                        } else {
                            h.task.clone()
                        }
                    })
                    .collect();
                // Pre-compute (total, done) per plan_id in one pass to avoid O(N*M) double-iteration.
                let mut wp_stats: std::collections::HashMap<uuid::Uuid, (usize, usize)> =
                    std::collections::HashMap::new();
                for wp in &state.work_packages {
                    if let Some(pid) = wp.plan_id {
                        let entry = wp_stats.entry(pid).or_default();
                        entry.0 += 1;
                        if wp.status == uglyhat::model::WorkPackageStatus::Done {
                            entry.1 += 1;
                        }
                    }
                }
                // Plans: (id, intent_preview, done_count, total_count)
                let plans: Vec<(uuid::Uuid, String, usize, usize)> = state
                    .plans
                    .iter()
                    .map(|p| {
                        let (total, done) =
                            wp_stats.get(&p.id).copied().unwrap_or_default();
                        let preview = if p.intent.len() > 36 {
                            format!("{}…", &p.intent[..36])
                        } else {
                            p.intent.clone()
                        };
                        (p.id, preview, done, total)
                    })
                    .collect();
                (threads, agents, pending_handoffs, unread_inbox, plans)
            })
            .unwrap_or_default();

        let workspace = self.workspace.clone();

        v_flex()
            .key_context("HqNavigator")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            // Status bar: uglyhat + gateway health
            .child({
                let uh_ok = self
                    .hq_state
                    .as_ref()
                    .map(|s| s.read(cx).error.is_none())
                    .unwrap_or(false);
                let gateway_url = std::env::var("PRISM_GATEWAY_URL").ok();
                let hq_state = self.hq_state.clone();
                h_flex()
                    .px_2()
                    .py_0p5()
                    .h(px(20.))
                    .gap_1()
                    .bg(cx.theme().colors().surface_background)
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        div()
                            .id("nav-status-uh-dot")
                            .w(px(6.))
                            .h(px(6.))
                            .rounded_full()
                            .flex_none()
                            .bg(if uh_ok {
                                Color::Success.color(cx)
                            } else {
                                Color::Error.color(cx)
                            })
                            .tooltip(Tooltip::text(if uh_ok {
                                "uglyhat connected"
                            } else {
                                "uglyhat not connected"
                            })),
                    )
                    .child(Label::new("uh").size(LabelSize::XSmall).color(if uh_ok {
                        Color::Muted
                    } else {
                        Color::Error
                    }))
                    .child(
                        div()
                            .id("nav-status-gw-dot")
                            .w(px(6.))
                            .h(px(6.))
                            .rounded_full()
                            .flex_none()
                            .bg(if gateway_url.is_some() {
                                Color::Success.color(cx)
                            } else {
                                Color::Muted.color(cx)
                            })
                            .tooltip(Tooltip::text(
                                gateway_url
                                    .clone()
                                    .unwrap_or_else(|| "gateway not configured".to_string()),
                            )),
                    )
                    .child(Label::new("gw").size(LabelSize::XSmall).color(Color::Muted))
                    .flex_1()
                    .when_some(hq_state.as_ref().map(|s| s.read(cx)), |this, hq| {
                        let agent_count = hq.agents.len();
                        let thread_count = hq.threads.len();
                        let handoff_count = hq
                            .handoffs
                            .iter()
                            .filter(|h| {
                                matches!(
                                    h.status,
                                    uglyhat::model::HandoffStatus::Pending
                                        | uglyhat::model::HandoffStatus::Running
                                )
                            })
                            .count();
                        this.child(
                            Label::new(format!("{agent_count}a {thread_count}t {handoff_count}h"))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
            })
            // Header with dispatch button
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .h(px(32.))
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .gap_1()
                    .child(
                        Label::new("Agent HQ")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .flex_1()
                    .child(
                        Button::new("new-task", "+ New Task")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(ws_ref) =
                                    this.workspace.as_ref().and_then(|w| w.upgrade())
                                {
                                    ws_ref.update(cx, |workspace, cx| {
                                        PlanDispatchModal::open(workspace, window, cx);
                                    });
                                }
                            })),
                    ),
            )
            // INBOX row
            .child({
                let hq_state_for_inbox = self.hq_state.clone();
                let ws_for_inbox = workspace.clone();
                h_flex()
                    .id("nav-inbox-row")
                    .px_2()
                    .py_1()
                    .h(px(28.))
                    .gap_1()
                    .w_full()
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().colors().element_hover))
                    .on_click(cx.listener(move |_, _, window, cx| {
                        if let Some(ws_ref) = ws_for_inbox.as_ref().and_then(|w| w.upgrade()) {
                            if let Some(ref hq) = hq_state_for_inbox {
                                let hq = hq.clone();
                                ws_ref.update(cx, |workspace, cx| {
                                    open_inbox(workspace, hq, window, cx);
                                });
                            }
                        }
                    }))
                    .child(
                        Label::new("Inbox")
                            .size(LabelSize::Small)
                            .color(Color::Default),
                    )
                    .flex_1()
                    .when(unread_inbox > 0, |this| {
                        this.child(
                            Label::new(format!("{unread_inbox}"))
                                .size(LabelSize::XSmall)
                                .color(Color::Error),
                        )
                    })
            })
            // PLANS section
            .when(!plans.is_empty(), |this| {
                let hq_for_plans = self.hq_state.clone();
                let ws_for_plans = workspace.clone();
                this.child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .h(px(24.))
                        .border_t_1()
                        .border_color(cx.theme().colors().border)
                        .gap_1()
                        .child(
                            Label::new("PLANS")
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new(format!("{}", plans.len()))
                                .size(LabelSize::XSmall)
                                .color(Color::Accent),
                        ),
                )
                .child(v_flex().id("plans-nav-list").px_2().gap_0p5().children(
                    plans.into_iter().enumerate().map(
                        |(ix, (plan_id, preview, done, total)): (
                            usize,
                            (uuid::Uuid, String, usize, usize),
                        )| {
                            let ws = ws_for_plans.clone();
                            let hq = hq_for_plans.clone();
                            h_flex()
                                .id(("nav-plan", ix))
                                .w_full()
                                .px_1()
                                .py_0p5()
                                .rounded_sm()
                                .cursor_pointer()
                                .hover(|s| s.bg(cx.theme().colors().element_hover))
                                .on_click(cx.listener(move |_, _, window, cx| {
                                    if let (Some(ws_ref), Some(hq_ref)) =
                                        (ws.as_ref().and_then(|w| w.upgrade()), hq.clone())
                                    {
                                        ws_ref.update(cx, |workspace, cx| {
                                            open_plan_view(
                                                workspace,
                                                hq_ref,
                                                Some(plan_id),
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .child(Label::new(preview).size(LabelSize::XSmall)),
                                )
                                .child(
                                    Label::new(format!("{done}/{total}"))
                                        .size(LabelSize::XSmall)
                                        .color(if done == total && total > 0 {
                                            Color::Success
                                        } else {
                                            Color::Muted
                                        }),
                                )
                        },
                    ),
                ))
            })
            // THREADS section
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .h(px(24.))
                    .border_t_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Label::new("THREADS")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(v_flex().id("threads-list").px_2().gap_0p5().children(
                threads.into_iter().enumerate().map(
                    |(ix, (name, agent_count)): (usize, (String, usize))| {
                        let ws = workspace.clone();
                        let thread_name = name.clone();
                        h_flex()
                            .id(("nav-thread", ix))
                            .w_full()
                            .px_1()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(cx.theme().colors().element_hover))
                            .on_click(cx.listener(move |_, _, window, cx| {
                                if let Some(ws_ref) = ws.as_ref().and_then(|w| w.upgrade()) {
                                    ws_ref.update(cx, |workspace, cx| {
                                        open_thread_view(
                                            workspace,
                                            thread_name.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }))
                            .child(Label::new(name).size(LabelSize::Small))
                            .flex_1()
                            .when(agent_count > 0, |this| {
                                this.child(
                                    Label::new(format!("{}", agent_count))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Accent),
                                )
                            })
                    },
                ),
            ))
            // AGENTS section
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .h(px(24.))
                    .border_t_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Label::new("AGENTS")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(v_flex().id("agents-nav-list").px_2().gap_0p5().children(
                agents.into_iter().enumerate().map(
                    |(ix, (name, state, current_thread)): (
                        usize,
                        (String, uglyhat::model::AgentState, Option<String>),
                    )| {
                        let ws = workspace.clone();
                        let agent_name = name.clone();
                        let state_color = match state {
                            uglyhat::model::AgentState::Working => Color::Accent,
                            uglyhat::model::AgentState::Idle => Color::Success,
                            uglyhat::model::AgentState::Blocked => Color::Warning,
                            uglyhat::model::AgentState::Dead => Color::Muted,
                        };
                        let is_spawned = RunningAgents::global(cx)
                            .map(|ra| ra.read(cx).is_running(&name))
                            .unwrap_or(false);
                        let unread = self
                            .hq_state
                            .as_ref()
                            .and_then(|s| s.read(cx).unread_by_agent.get(&name).copied())
                            .unwrap_or(0);
                        v_flex()
                            .id(("nav-agent", ix))
                            .w_full()
                            .px_1()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(cx.theme().colors().element_hover))
                            .on_click(cx.listener(move |_, _, window, cx| {
                                if let Some(ws_ref) = ws.as_ref().and_then(|w| w.upgrade()) {
                                    ws_ref.update(cx, |workspace, cx| {
                                        open_agent_view(workspace, agent_name.clone(), window, cx);
                                    });
                                }
                            }))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Label::new("●").size(LabelSize::XSmall).color(state_color),
                                    )
                                    .child(Label::new(name).size(LabelSize::Small))
                                    .flex_1()
                                    .when(is_spawned, |this| {
                                        this.child(
                                            Label::new("◉")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Accent),
                                        )
                                    })
                                    .when(unread > 0, |this| {
                                        this.child(
                                            Label::new(format!("{}", unread))
                                                .size(LabelSize::XSmall)
                                                .color(Color::Error),
                                        )
                                    }),
                            )
                            .when_some(current_thread, |this, thread| {
                                this.child(
                                    Label::new(thread)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            })
                    },
                ),
            ))
            // HANDOFFS section
            .when(!handoffs.is_empty(), |this| {
                this.child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .h(px(24.))
                        .border_t_1()
                        .border_color(cx.theme().colors().border)
                        .gap_1()
                        .child(
                            Label::new("HANDOFFS")
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new(format!("{}", handoffs.len()))
                                .size(LabelSize::XSmall)
                                .color(Color::Warning),
                        ),
                )
                .child(
                    v_flex().id("handoffs-nav-list").px_2().gap_0p5().children(
                        handoffs
                            .into_iter()
                            .enumerate()
                            .map(|(ix, task): (usize, String)| {
                                h_flex()
                                    .id(("nav-handoff", ix))
                                    .w_full()
                                    .px_1()
                                    .py_0p5()
                                    .gap_1()
                                    .child(
                                        Label::new("↗")
                                            .size(LabelSize::XSmall)
                                            .color(Color::Warning),
                                    )
                                    .child(Label::new(task).size(LabelSize::XSmall))
                            }),
                    ),
                )
            })
    }
}
