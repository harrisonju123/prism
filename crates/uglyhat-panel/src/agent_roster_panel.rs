use crate::types::{uh_binary, AgentStatus, WorkspaceContext};
use anyhow::Result;
use db::kvp::KEY_VALUE_STORE;
use gpui::{
    actions, px, Action, App, AsyncWindowContext, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, IntoElement, ParentElement, Pixels, Render, Styled, Task, WeakEntity,
    Window,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use ui::{
    h_flex, prelude::*, v_flex, Button, ButtonStyle, Color, IconButton, IconName, Label, LabelSize,
    Tooltip,
};
use util::{ResultExt, TryFutureExt};
use workspace::{
    dock::{DockPosition, Panel, PanelEvent},
    Workspace,
};

const PANEL_KEY: &str = "AgentRosterPanel";
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

actions!(
    agent_roster,
    [
        /// Toggles the agent roster panel.
        Toggle,
        /// Toggles focus on the agent roster panel.
        ToggleFocus
    ]
);

#[derive(Default)]
enum ViewState {
    #[default]
    Roster,
    Messaging {
        agent: AgentStatus,
        task_id: String,
        compose_text: String,
        sending: bool,
        sent_ok: bool,
    },
}

pub struct AgentRosterPanel {
    focus_handle: FocusHandle,
    width: Option<Pixels>,
    active: bool,
    agents: Vec<AgentStatus>,
    is_loading: bool,
    error: Option<String>,
    view_state: ViewState,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
    pending_serialization: Task<Option<()>>,
    send_task: Option<Task<()>>,
}

#[derive(Serialize, Deserialize)]
struct SerializedPanel {
    width: Option<Pixels>,
}

#[derive(Debug)]
pub enum Event {
    DockPositionChanged,
    Focus,
    Dismissed,
}

impl AgentRosterPanel {
    pub fn new(
        _workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let mut panel = Self {
                focus_handle: cx.focus_handle(),
                width: None,
                active: false,
                agents: Vec::new(),
                is_loading: false,
                error: None,
                view_state: ViewState::Roster,
                refresh_task: None,
                _auto_refresh: Task::ready(()),
                pending_serialization: Task::ready(None),
                send_task: None,
            };

            let auto_refresh = cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                this.update(cx, |panel: &mut AgentRosterPanel, cx| {
                    if panel.active {
                        panel.refresh(cx);
                    }
                })
                .ok();
            });
            panel._auto_refresh = auto_refresh;
            panel.refresh(cx);
            panel
        })
    }

    pub fn load(
        workspace: WeakEntity<Workspace>,
        cx: AsyncWindowContext,
    ) -> Task<Result<Entity<Self>>> {
        cx.spawn(async move |cx| {
            let serialized_panel = cx
                .background_spawn(async move { KEY_VALUE_STORE.read_kvp(PANEL_KEY) })
                .await
                .log_err()
                .flatten()
                .and_then(|s| serde_json::from_str::<SerializedPanel>(&s).log_err());

            workspace.update_in(cx, |workspace, window, cx| {
                let panel = Self::new(workspace, window, cx);
                if let Some(serialized) = serialized_panel {
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
                        serde_json::to_string(&SerializedPanel { width })?,
                    )
                    .await?;
                anyhow::Ok(())
            }
            .log_err(),
        );
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.is_loading = true;
        self.error = None;
        cx.notify();

        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let output = std::process::Command::new(uh_binary())
                        .arg("context")
                        .output()?;
                    if !output.status.success() {
                        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr));
                    }
                    let ctx = serde_json::from_slice::<WorkspaceContext>(&output.stdout)?;
                    anyhow::Ok(ctx.active_agents)
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok(agents) => {
                        this.agents = agents;
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

    fn open_messaging(&mut self, agent: AgentStatus, cx: &mut Context<Self>) {
        let task_id = agent.current_task_id.clone().unwrap_or_default();
        self.view_state = ViewState::Messaging {
            agent,
            task_id,
            compose_text: String::new(),
            sending: false,
            sent_ok: false,
        };
        cx.notify();
    }

    fn close_messaging(&mut self, cx: &mut Context<Self>) {
        self.view_state = ViewState::Roster;
        self.send_task = None;
        self.error = None;
        cx.notify();
    }

    fn send_message(&mut self, cx: &mut Context<Self>) {
        let ViewState::Messaging {
            ref agent,
            ref task_id,
            ref compose_text,
            ref mut sending,
            ..
        } = self.view_state
        else {
            return;
        };

        if compose_text.trim().is_empty() {
            return;
        }

        *sending = true;
        cx.notify();

        let title = format!("Message to {}", agent.name);
        let content = compose_text.trim().to_string();
        let task_id_arg = if task_id.is_empty() {
            None
        } else {
            Some(task_id.clone())
        };

        self.send_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut cmd = std::process::Command::new(uh_binary());
                    cmd.args(["note", &title, "--content", &content]);
                    if let Some(tid) = task_id_arg {
                        cmd.args(["--task-id", &tid]);
                    }
                    let output = cmd.output()?;
                    if !output.status.success() {
                        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr));
                    }
                    anyhow::Ok(())
                })
                .await;

            this.update(cx, |this, cx| {
                this.send_task = None;
                if let ViewState::Messaging {
                    ref mut sending,
                    ref mut sent_ok,
                    ref mut compose_text,
                    ..
                } = this.view_state
                {
                    *sending = false;
                    match result {
                        Ok(()) => {
                            *sent_ok = true;
                            *compose_text = String::new();
                        }
                        Err(e) => {
                            this.error = Some(e.to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_agent_row(&self, agent: &AgentStatus, cx: &mut Context<Self>) -> impl IntoElement {
        let dot_color = if agent.session_open {
            Color::Success
        } else {
            Color::Muted
        };
        let task_label = agent
            .current_task_name
            .clone()
            .unwrap_or_else(|| "idle".to_owned());
        let task_color = if agent.current_task_name.is_some() {
            Color::Default
        } else {
            Color::Muted
        };
        let agent_clone = agent.clone();

        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .hover(|style| style.bg(cx.theme().colors().element_hover))
            .child(
                div()
                    .w(px(8.))
                    .h(px(8.))
                    .rounded_full()
                    .flex_none()
                    .bg(dot_color.color(cx)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Label::new(agent.name.clone())
                            .size(LabelSize::Small)
                            .truncate(),
                    )
                    .child(
                        Label::new(task_label)
                            .size(LabelSize::Small)
                            .color(task_color)
                            .truncate(),
                    ),
            )
            .when(agent.session_open, |this| {
                this.child(
                    IconButton::new(
                        ElementId::Name(format!("msg-{}", agent_clone.name).into()),
                        IconName::Chat,
                    )
                    .icon_size(ui::IconSize::Small)
                    .icon_color(Color::Muted)
                    .tooltip(Tooltip::text("Send message"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_messaging(agent_clone.clone(), cx);
                    })),
                )
            })
    }

    fn render_messaging_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let ViewState::Messaging {
            ref agent,
            ref compose_text,
            ref sending,
            ref sent_ok,
            ..
        } = self.view_state
        else {
            return v_flex().into_any_element();
        };

        let agent_name = agent.name.clone();
        let task_name = agent.current_task_name.clone();
        let is_sending = *sending;
        let was_sent = *sent_ok;
        let compose_text = compose_text.clone();

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .h(px(32.))
                    .flex_none()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        IconButton::new("back", IconName::ArrowLeft)
                            .icon_size(ui::IconSize::Small)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Back to roster"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_messaging(cx))),
                    )
                    .child(
                        Label::new(format!("→ {agent_name}"))
                            .size(LabelSize::Small)
                            .truncate(),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .when_some(task_name, |this, t| {
                        this.child(
                            Label::new(format!("Task: {t}"))
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        )
                    })
                    .child(
                        Label::new("Note will be attached to agent's current task.")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .rounded_md()
                            .min_h(px(80.))
                            .child(
                                Label::new(if compose_text.is_empty() {
                                    "Type your message…".to_string()
                                } else {
                                    compose_text.clone()
                                })
                                .size(LabelSize::Small)
                                .color(
                                    if compose_text.is_empty() {
                                        Color::Muted
                                    } else {
                                        Color::Default
                                    },
                                ),
                            ),
                    )
                    .when(was_sent, |this| {
                        this.child(
                            Label::new("Message sent.")
                                .size(LabelSize::Small)
                                .color(Color::Success),
                        )
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .flex_none()
                    .border_t_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Button::new(
                            "send-msg",
                            if is_sending {
                                "Sending…"
                            } else {
                                "Send Note"
                            },
                        )
                        .style(ButtonStyle::Filled)
                        .label_size(LabelSize::Small)
                        .disabled(is_sending || compose_text.trim().is_empty())
                        .on_click(cx.listener(|this, _, _, cx| this.send_message(cx))),
                    )
                    .child(
                        Button::new("cancel-msg", "Cancel")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.close_messaging(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl EventEmitter<Event> for AgentRosterPanel {}
impl EventEmitter<PanelEvent> for AgentRosterPanel {}

impl Focusable for AgentRosterPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AgentRosterPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_messaging = matches!(&self.view_state, ViewState::Messaging { .. });

        if is_messaging {
            return v_flex()
                .key_context("AgentRoster")
                .track_focus(&self.focus_handle)
                .size_full()
                .child(self.render_messaging_view(cx))
                .into_any_element();
        }

        let agents = self.agents.clone();
        let active_count = agents.iter().filter(|a| a.session_open).count();

        v_flex()
            .key_context("AgentRoster")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                h_flex()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .h(px(32.))
                    .flex_none()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Label::new(format!("Agents ({active_count} active)"))
                            .size(LabelSize::Small),
                    )
                    .child(
                        IconButton::new("refresh", IconName::ArrowCircle)
                            .icon_size(ui::IconSize::Small)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                v_flex()
                    .id("roster-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .when_some(self.error.clone(), |this, err| {
                        this.child(
                            div()
                                .px_2()
                                .py_1()
                                .child(Label::new(err).size(LabelSize::Small).color(Color::Error)),
                        )
                    })
                    .when(self.is_loading && agents.is_empty(), |this| {
                        this.child(
                            div().px_2().py_1().child(
                                Label::new("Loading…")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                        )
                    })
                    .when(agents.is_empty() && !self.is_loading, |this| {
                        this.child(
                            div().px_2().py_1().child(
                                Label::new(
                                    "No agents registered. Run `uh checkin` to appear here.",
                                )
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                            ),
                        )
                    })
                    .children(agents.iter().map(|agent| self.render_agent_row(agent, cx))),
            )
            .into_any_element()
    }
}

impl Panel for AgentRosterPanel {
    fn persistent_name() -> &'static str {
        "AgentRosterPanel"
    }

    fn panel_key() -> &'static str {
        PANEL_KEY
    }

    fn position(&self, _: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, _position: DockPosition, _: &mut Window, _cx: &mut Context<Self>) {}

    fn size(&self, _: &Window, _cx: &App) -> Pixels {
        self.width.unwrap_or(px(280.))
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
        Some(IconName::Person)
    }

    fn icon_tooltip(&self, _: &Window, _cx: &App) -> Option<&'static str> {
        Some("Agent Roster")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        9
    }
}
