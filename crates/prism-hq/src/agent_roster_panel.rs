use crate::context_service::get_context_handle;
use crate::panel_types::{prism_binary, AgentStatus};
use anyhow::Result;
use db::kvp::KEY_VALUE_STORE;
use gpui::{
    actions, px, Action, App, AsyncWindowContext, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, IntoElement, ParentElement, Pixels, Render, Styled, Task, WeakEntity,
    Window,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use ui::{
    h_flex, prelude::*, v_flex, Button, ButtonStyle, Color, IconButton, IconName, Label, LabelSize,
    Tooltip,
};
use util::{ResultExt, TryFutureExt};
use workspace::{
    dock::{DockPosition, Panel, PanelEvent},
    open_paths, AppState, OpenOptions, Workspace,
};

const PANEL_KEY: &str = "AgentRosterPanel";
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

actions!(
    agent_roster,
    [
        /// Toggles the agent roster panel.
        Toggle,
        /// Toggles focus on the agent roster panel.
        ToggleFocus,
        /// Spawns a new Claude Code agent in a new git worktree and opens a Zed window there.
        SpawnAgentInWorktree,
        /// Opens a picker to select an existing worktree from .worktrees/.
        PickWorktree,
        /// Sets the agent name for this session (persisted to KV store).
        SetAgentName,
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
    spawn_error: Option<String>,
    spawn_task: Option<Task<()>>,
    /// Persisted agent name (mirrors UH_AGENT_NAME env var).
    agent_name: Option<String>,
    /// Whether the agent name input row is visible.
    editing_agent_name: bool,
    /// Worktrees available for picking (populated by pick_worktree).
    available_worktrees: Vec<(String, std::path::PathBuf)>,
    /// Whether the worktree picker modal is shown.
    show_worktree_picker: bool,
    pick_task: Option<Task<()>>,
    /// Cached app_state for opening selected worktrees.
    cached_app_state: Option<Arc<AppState>>,
}

#[derive(Serialize, Deserialize)]
struct SerializedPanel {
    width: Option<Pixels>,
    agent_name: Option<String>,
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
            let agent_name = std::env::var("PRISM_AGENT_NAME").or_else(|_| std::env::var("UH_AGENT_NAME")).ok();
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
                spawn_error: None,
                spawn_task: None,
                agent_name,
                editing_agent_name: false,
                available_worktrees: Vec::new(),
                show_worktree_picker: false,
                pick_task: None,
                cached_app_state: None,
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
                        if let Some(name) = serialized.agent_name {
                            // Restore env var from persisted name
                            // SAFETY: single-threaded GUI context; set_var is safe here.
                            unsafe { std::env::set_var("PRISM_AGENT_NAME", &name) };
                            panel.agent_name = Some(name);
                        }
                        cx.notify();
                    });
                }
                panel
            })
        })
    }

    fn serialize(&mut self, cx: &mut Context<Self>) {
        let width = self.width;
        let agent_name = self.agent_name.clone();
        self.pending_serialization = cx.background_spawn(
            async move {
                KEY_VALUE_STORE
                    .write_kvp(
                        PANEL_KEY.into(),
                        serde_json::to_string(&SerializedPanel { width, agent_name })?,
                    )
                    .await?;
                anyhow::Ok(())
            }
            .log_err(),
        );
    }

    fn set_agent_name(&mut self, name: String, cx: &mut Context<Self>) {
        // SAFETY: single-threaded GUI context; set_var is safe here.
        unsafe { std::env::set_var("PRISM_AGENT_NAME", &name) };
        self.agent_name = Some(name);
        self.editing_agent_name = false;
        self.serialize(cx);
        cx.notify();
    }

    pub fn toggle_agent_name_input(&mut self, cx: &mut Context<Self>) {
        self.editing_agent_name = !self.editing_agent_name;
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.is_loading = true;
        self.error = None;
        cx.notify();

        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let handle = get_context_handle(&this, cx);

            let result = cx
                .background_spawn(async move {
                    let Some(handle) = handle else {
                        anyhow::bail!("context service not available");
                    };
                    let agents = handle.list_agents()?;
                    let panel_agents: Vec<AgentStatus> =
                        agents.into_iter().map(AgentStatus::from).collect();
                    anyhow::Ok(panel_agents)
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

        self.send_task = Some(cx.spawn(async move |this, cx| {
            let handle = get_context_handle(&this, cx);
            let result = cx
                .background_spawn(async move {
                    let Some(handle) = handle else {
                        anyhow::bail!("context service not available");
                    };
                    handle.save_memory(&title, &content, None, vec!["note".to_string()])?;
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

    pub fn spawn_worktree_agent(
        &mut self,
        app_state: Arc<AppState>,
        repo_root: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.spawn_error = None;
        cx.notify();

        self.spawn_task = Some(cx.spawn(async move |this, cx| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let secs = now.as_secs();
            let name = format!("agent-{secs}");
            let wt_path = repo_root.join(".worktrees").join(&name);

            let result: anyhow::Result<()> = async {
                let git_status = cx
                    .background_spawn({
                        let repo_root = repo_root.clone();
                        let name = name.clone();
                        let wt_path = wt_path.clone();
                        async move {
                            std::process::Command::new("git")
                                .arg("worktree")
                                .arg("add")
                                .arg(&wt_path)
                                .arg("-b")
                                .arg(&name)
                                .current_dir(&repo_root)
                                .status()
                        }
                    })
                    .await?;
                if !git_status.success() {
                    anyhow::bail!("git worktree add failed");
                }

                cx.background_spawn({
                    let repo_root = repo_root.clone();
                    let name = name.clone();
                    async move {
                        std::process::Command::new(prism_binary())
                            .args(["context", "thread", "create", &name])
                            .current_dir(&repo_root)
                            .status()
                            .ok();
                    }
                })
                .await;

                let open_task = cx.update(|app_cx| {
                    open_paths(&[wt_path.clone()], app_state, OpenOptions::default(), app_cx)
                });
                open_task.await?;

                // Launch prism-cli agent in the new worktree (fire-and-forget)
                cx.background_spawn({
                    let wt_path = wt_path.clone();
                    let name = name.clone();
                    async move {
                        std::process::Command::new(prism_binary())
                            .args([
                                "run",
                                "--model",
                                "claude-sonnet-4-6",
                                &format!(
                                    "You are agent '{}'. Explore the codebase and await instructions.",
                                    name
                                ),
                            ])
                            .current_dir(&wt_path)
                            .env("UH_AGENT_NAME", &name)
                            .spawn()
                            .log_err();
                    }
                })
                .detach();

                anyhow::Ok(())
            }
            .await;

            this.update(cx, |this, cx| {
                this.spawn_task = None;
                match result {
                    Ok(()) => {
                        this.refresh(cx);
                    }
                    Err(e) => {
                        this.spawn_error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn pick_worktree(
        &mut self,
        app_state: Arc<AppState>,
        repo_root: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.show_worktree_picker = false;
        self.cached_app_state = Some(app_state.clone());
        cx.notify();

        self.pick_task = Some(cx.spawn(async move |this, cx| {
            let entries = cx
                .background_spawn({
                    let repo_root = repo_root.clone();
                    async move {
                        let wt_dir = repo_root.join(".worktrees");
                        std::fs::read_dir(&wt_dir)
                            .into_iter()
                            .flatten()
                            .filter_map(|e| e.ok())
                            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                            .collect::<Vec<_>>()
                    }
                })
                .await;

            this.update(cx, |this, cx| {
                this.pick_task = None;
                this.available_worktrees = entries;
                this.show_worktree_picker = true;
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_worktree(&mut self, path: PathBuf, app_state: Arc<AppState>, cx: &mut Context<Self>) {
        self.show_worktree_picker = false;
        cx.notify();

        self.spawn_task = Some(cx.spawn(async move |this, cx| {
            let result: anyhow::Result<()> = async {
                let open_task = cx.update(|app_cx| {
                    open_paths(&[path], app_state, OpenOptions::default(), app_cx)
                });
                open_task.await?;
                anyhow::Ok(())
            }
            .await;

            this.update(cx, |this, cx| {
                this.spawn_task = None;
                this.cached_app_state = None;
                if let Err(e) = result {
                    this.spawn_error = Some(e.to_string());
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
        let agent_rows: Vec<gpui::AnyElement> = agents.iter().map(|agent| self.render_agent_row(agent, cx).into_any_element()).collect();

        let agent_name_label = self
            .agent_name
            .clone()
            .unwrap_or_else(|| "no agent name set".to_string());

        // Worktree picker modal overlay
        if self.show_worktree_picker {
            let worktrees = self.available_worktrees.clone();
            return v_flex()
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
                        .child(Label::new("Select Worktree").size(LabelSize::Small))
                        .child(
                            IconButton::new("close-picker", IconName::Close)
                                .icon_size(ui::IconSize::Small)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text("Cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_worktree_picker = false;
                                    this.cached_app_state = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    v_flex()
                        .id("wt-picker-list")
                        .flex_1()
                        .overflow_y_scroll()
                        .when(worktrees.is_empty(), |this| {
                            this.child(
                                div().px_2().py_1().child(
                                    Label::new("No worktrees found in .worktrees/")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                            )
                        })
                        .children(
                            worktrees
                                .into_iter()
                                .enumerate()
                                .map(|(idx, (name, path))| {
                                    let path_clone = path.clone();
                                    let name_label = name.clone();
                                    h_flex()
                                        .id(ElementId::Name(format!("wt-item-{idx}").into()))
                                        .w_full()
                                        .px_2()
                                        .py_1()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(cx.theme().colors().element_hover))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(app_state) = this.cached_app_state.clone() {
                                                this.open_worktree(
                                                    path_clone.clone(),
                                                    app_state,
                                                    cx,
                                                );
                                            }
                                        }))
                                        .child(Label::new(name_label).size(LabelSize::Small))
                                }),
                        ),
                )
                .into_any_element();
        }

        v_flex()
            .key_context("AgentRoster")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                v_flex()
                    .px_2()
                    .pt_1()
                    .flex_none()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        h_flex()
                            .justify_between()
                            .h(px(28.))
                            .child(
                                Label::new(format!("Agents ({active_count} active)"))
                                    .size(LabelSize::Small),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        IconButton::new("pick-worktree", IconName::FileTree)
                                            .icon_size(ui::IconSize::Small)
                                            .icon_color(Color::Muted)
                                            .tooltip(Tooltip::text("Open existing worktree"))
                                            .on_click(cx.listener(|_, _, window, cx| {
                                                window.dispatch_action(
                                                    PickWorktree.boxed_clone(),
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        IconButton::new("spawn-agent", IconName::Plus)
                                            .icon_size(ui::IconSize::Small)
                                            .icon_color(Color::Muted)
                                            .tooltip(Tooltip::text("Spawn agent in new worktree"))
                                            .disabled(self.spawn_task.is_some())
                                            .on_click(cx.listener(|_, _, window, cx| {
                                                window.dispatch_action(
                                                    SpawnAgentInWorktree.boxed_clone(),
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        IconButton::new("refresh", IconName::ArrowCircle)
                                            .icon_size(ui::IconSize::Small)
                                            .icon_color(Color::Muted)
                                            .tooltip(Tooltip::text("Refresh"))
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.refresh(cx)),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .h(px(20.))
                            .pb_1()
                            .gap_1()
                            .child(
                                Label::new(format!("Agent: {agent_name_label}"))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(
                                IconButton::new("set-agent-name", IconName::Pencil)
                                    .icon_size(ui::IconSize::XSmall)
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text("Set agent name (reads PRISM_AGENT_NAME)"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        // Re-read PRISM_AGENT_NAME from env (user may have set it externally)
                                        if let Ok(name) = std::env::var("PRISM_AGENT_NAME").or_else(|_| std::env::var("UH_AGENT_NAME")) {
                                            this.set_agent_name(name, cx);
                                        } else {
                                            this.toggle_agent_name_input(cx);
                                        }
                                    })),
                            ),
                    )
                    .when(self.editing_agent_name, |this| {
                        this.child(
                            div().px_2().pb_1().child(
                                Label::new("Set PRISM_AGENT_NAME env var, then click ✎ to refresh.")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                        )
                    }),
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
                    .when_some(self.spawn_error.clone(), |this, err| {
                        this.child(
                            div().px_2().py_1().child(
                                Label::new(format!("Spawn failed: {err}"))
                                    .size(LabelSize::Small)
                                    .color(Color::Error),
                            ),
                        )
                    })
                    .when(self.spawn_task.is_some(), |this| {
                        this.child(
                            div().px_2().py_1().child(
                                Label::new("Spawning agent…")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
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
                                    "No agents registered. Run `prism context checkin` to appear here.",
                                )
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                            ),
                        )
                    })
                    .children(agent_rows),
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
