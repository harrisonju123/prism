use crate::types::{AgentStatus, DependencyInfo, StatusCount, TaskContext, TaskSummary, WorkspaceContext};
use anyhow::Result;
use db::kvp::KEY_VALUE_STORE;
use gpui::{
    Action, App, AsyncWindowContext, ClickEvent, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, IntoElement, ParentElement, Pixels, Render, Styled, Task, WeakEntity,
    Window, actions, px,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use ui::{
    Button, ButtonStyle, Color, Icon, IconButton, IconName, Label, LabelSize, Tooltip, h_flex,
    prelude::*, v_flex,
};
use util::{ResultExt, TryFutureExt};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

const TASK_BOARD_PANEL_KEY: &str = "TaskBoardPanel";
const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

fn uh_binary() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let p = std::path::PathBuf::from(home).join(".cargo/bin/uh");
        if p.exists() {
            return p;
        }
    }
    "uh".into()
}

actions!(
    uglyhat_panel,
    [
        /// Toggles the task board panel.
        Toggle,
        /// Toggles focus on the task board panel.
        ToggleFocus
    ]
);

#[derive(Default)]
enum ViewState {
    #[default]
    Board,
    LoadingDetail {
        task_id: String,
    },
    Detail(Box<TaskContext>),
}

pub struct TaskBoardPanel {
    focus_handle: FocusHandle,
    width: Option<Pixels>,
    active: bool,
    context: Option<WorkspaceContext>,
    next_tasks: Vec<TaskSummary>,
    is_loading: bool,
    error: Option<String>,
    agents_expanded: bool,
    stale_expanded: bool,
    active_expanded: bool,
    next_expanded: bool,
    refresh_task: Option<Task<()>>,
    pending_serialization: Task<Option<()>>,
    _auto_refresh: Task<()>,
    // Navigation
    view_state: ViewState,
    detail_task: Option<Task<()>>,
    // Check-in/checkout
    agent_name: Option<String>,
    checkin_task: Option<Task<()>>,
    // Cost section
    cost_expanded: bool,
    prism_api_key: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SerializedTaskBoardPanel {
    width: Option<Pixels>,
    #[serde(default)]
    cost_expanded: bool,
}

#[derive(Debug)]
pub enum Event {
    DockPositionChanged,
    Focus,
    Dismissed,
}

impl TaskBoardPanel {
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
                context: None,
                next_tasks: Vec::new(),
                is_loading: false,
                error: None,
                agents_expanded: true,
                stale_expanded: true,
                active_expanded: true,
                next_expanded: true,
                refresh_task: None,
                pending_serialization: Task::ready(None),
                _auto_refresh: Task::ready(()),
                view_state: ViewState::Board,
                detail_task: None,
                agent_name: std::env::var("UH_AGENT_NAME").ok(),
                checkin_task: None,
                cost_expanded: false,
                prism_api_key: std::env::var("PRISM_API_KEY").ok(),
            };

            let auto_refresh = cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(AUTO_REFRESH_INTERVAL).await;
                this.update(cx, |panel: &mut TaskBoardPanel, cx| panel.refresh(cx))
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
                .background_spawn(async move {
                    KEY_VALUE_STORE.read_kvp(TASK_BOARD_PANEL_KEY)
                })
                .await
                .log_err()
                .flatten()
                .and_then(|panel| {
                    serde_json::from_str::<SerializedTaskBoardPanel>(&panel).log_err()
                });

            workspace.update_in(cx, |workspace, window, cx| {
                let panel = Self::new(workspace, window, cx);
                if let Some(serialized_panel) = serialized_panel {
                    panel.update(cx, |panel, cx| {
                        panel.width = serialized_panel.width.map(|w| w.round());
                        panel.cost_expanded = serialized_panel.cost_expanded;
                        cx.notify();
                    });
                }
                panel
            })
        })
    }

    fn serialize(&mut self, cx: &mut Context<Self>) {
        let width = self.width;
        let cost_expanded = self.cost_expanded;
        self.pending_serialization = cx.background_spawn(
            async move {
                KEY_VALUE_STORE
                    .write_kvp(
                        TASK_BOARD_PANEL_KEY.into(),
                        serde_json::to_string(&SerializedTaskBoardPanel {
                            width,
                            cost_expanded,
                        })?,
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
            let context_result = cx
                .background_spawn(async move {
                    let output = std::process::Command::new(uh_binary()).arg("context").output()?;
                    if !output.status.success() {
                        anyhow::bail!(
                            "{}",
                            String::from_utf8_lossy(&output.stderr)
                        );
                    }
                    serde_json::from_slice::<WorkspaceContext>(&output.stdout)
                        .map_err(anyhow::Error::from)
                })
                .await;

            let next_result = cx
                .background_spawn(async move {
                    let output = std::process::Command::new(uh_binary()).arg("next").output()?;
                    if !output.status.success() {
                        return Ok(Vec::new());
                    }
                    serde_json::from_slice::<Vec<TaskSummary>>(&output.stdout)
                        .map_err(anyhow::Error::from)
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match (context_result, next_result) {
                    (Ok(ctx), Ok(next)) => {
                        this.context = Some(ctx);
                        this.next_tasks = next;
                        this.error = None;
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        this.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_task_detail(&mut self, task_id: String, cx: &mut Context<Self>) {
        self.view_state = ViewState::LoadingDetail {
            task_id: task_id.clone(),
        };
        cx.notify();
        self.detail_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let output = std::process::Command::new(uh_binary())
                        .args(["task", "context", &task_id])
                        .output()?;
                    if !output.status.success() {
                        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr));
                    }
                    serde_json::from_slice::<TaskContext>(&output.stdout)
                        .map_err(anyhow::Error::from)
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(ctx) => {
                        this.view_state = ViewState::Detail(Box::new(ctx));
                    }
                    Err(e) => {
                        this.view_state = ViewState::Board;
                        this.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.view_state = ViewState::Board;
        self.detail_task = None;
        cx.notify();
    }

    fn run_checkin(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.agent_name.clone() else {
            return;
        };
        self.checkin_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let output = std::process::Command::new(uh_binary())
                        .args(["checkin", "--name", &name, "--capabilities", "zed"])
                        .output()?;
                    if !output.status.success() {
                        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr));
                    }
                    anyhow::Ok(())
                })
                .await;
            this.update(cx, |this, cx| {
                this.checkin_task = None;
                if let Err(e) = result {
                    this.error = Some(e.to_string());
                    cx.notify();
                } else {
                    this.refresh(cx);
                }
            })
            .ok();
        }));
    }

    fn run_checkout(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.agent_name.clone() else {
            return;
        };
        self.checkin_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let output = std::process::Command::new(uh_binary())
                        .args(["checkout", "--name", &name, "--summary", "Zed session ended"])
                        .output()?;
                    if !output.status.success() {
                        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr));
                    }
                    anyhow::Ok(())
                })
                .await;
            this.update(cx, |this, cx| {
                this.checkin_task = None;
                if let Err(e) = result {
                    this.error = Some(e.to_string());
                    cx.notify();
                } else {
                    this.refresh(cx);
                }
            })
            .ok();
        }));
    }

    fn priority_color(priority: &str) -> Color {
        match priority {
            "critical" => Color::Error,
            "high" => Color::Warning,
            "medium" => Color::Accent,
            _ => Color::Muted,
        }
    }

    fn render_task_row(task: &TaskSummary, cx: &App) -> gpui::Div {
        let color = Self::priority_color(&task.priority);
        let task_name = task.name.clone();
        let epic_name = task.epic_name.clone().unwrap_or_default();

        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .hover(|style| style.bg(cx.theme().colors().element_hover))
            .child(
                div()
                    .w(px(6.))
                    .h(px(6.))
                    .rounded_full()
                    .flex_none()
                    .bg(color.color(cx)),
            )
            .child(
                Label::new(task_name)
                    .size(LabelSize::Small)
                    .truncate(),
            )
            .when(!epic_name.is_empty(), |this| {
                this.child(
                    Label::new(epic_name)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
    }

    fn render_dep_row(dep: &DependencyInfo, color: Color, cx: &App) -> gpui::Div {
        h_flex()
            .w_full()
            .px_2()
            .py_0p5()
            .gap_2()
            .child(
                div()
                    .w(px(6.))
                    .h(px(6.))
                    .rounded_full()
                    .flex_none()
                    .bg(color.color(cx)),
            )
            .child(
                Label::new(dep.task_name.clone())
                    .size(LabelSize::Small)
                    .truncate(),
            )
            .child(
                Label::new(dep.status.replace('_', " "))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
    }

    fn render_agent_row(agent: &AgentStatus, my_name: &str, cx: &App) -> impl IntoElement {
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
        let name_color = if agent.name == my_name {
            Color::Accent
        } else {
            Color::Default
        };

        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .hover(|style| style.bg(cx.theme().colors().element_hover))
            .child(
                div()
                    .w(px(6.))
                    .h(px(6.))
                    .rounded_full()
                    .flex_none()
                    .bg(dot_color.color(cx)),
            )
            .child(Label::new(agent.name.clone()).size(LabelSize::Small).color(name_color))
            .child(
                Label::new(task_label)
                    .size(LabelSize::Small)
                    .color(task_color),
            )
    }

    fn render_summary_footer(status_counts: &[StatusCount]) -> impl IntoElement {
        let parts: Vec<String> = status_counts
            .iter()
            .map(|sc| format!("{} {}", sc.count, sc.status.replace('_', " ")))
            .collect();
        let summary = parts.join(" · ");

        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .border_t_1()
            .child(
                Label::new(summary)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
    }

    fn render_section_header(
        id: impl Into<ElementId>,
        label: &str,
        expanded: bool,
        on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .id(id)
            .w_full()
            .px_2()
            .py_1()
            .gap_1()
            .bg(cx.theme().colors().surface_background)
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .cursor_pointer()
            .on_click(on_toggle)
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(ui::IconSize::Small)
                .color(Color::Muted),
            )
            .child(
                Label::new(label.to_owned())
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
    }

    fn render_detail_view(&self, ctx: &TaskContext, cx: &mut Context<Self>) -> impl IntoElement {
        let task = &ctx.task;
        let status = task.status.replace('_', " ");
        let assignee = task
            .assignee
            .clone()
            .unwrap_or_else(|| "unassigned".to_owned());
        let initiative_name = task.initiative_name.clone();
        let epic_name = task.epic_name.clone();
        let description = task.description.clone();
        let blocked_by = ctx.blocked_by.clone();
        let blocks = ctx.blocks.clone();
        let notes = ctx.notes.clone();
        let handoffs = ctx.handoffs.clone();
        let recent_activity = ctx.recent_activity.clone();
        let task_name = task.name.clone();

        v_flex()
            .size_full()
            // Back header
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
                            .tooltip(Tooltip::text("Back to board"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_detail(cx))),
                    )
                    .child(
                        Label::new(task_name)
                            .size(LabelSize::Small)
                            .truncate(),
                    ),
            )
            // Scrollable body
            .child(
                v_flex()
                    .id("detail-body")
                    .flex_1()
                    .overflow_y_scroll()
                    // Status + assignee
                    .child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .child(Label::new(status).size(LabelSize::Small))
                            .child(Label::new("·").size(LabelSize::Small).color(Color::Muted))
                            .child(
                                Label::new(assignee)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    // Epic breadcrumb
                    .when_some(epic_name, |this: gpui::Stateful<gpui::Div>, e| {
                        this.child(
                            h_flex()
                                .px_2()
                                .gap_1()
                                .when_some(initiative_name, |this: gpui::Div, init| {
                                    this.child(
                                        Label::new(init)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        Label::new("›")
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                })
                                .child(Label::new(e).size(LabelSize::Small).color(Color::Muted)),
                        )
                    })
                    // Description
                    .when_some(description, |this: gpui::Stateful<gpui::Div>, desc| {
                        this.child(
                            v_flex()
                                .px_2()
                                .py_1()
                                .child(
                                    Label::new("Description")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .child(Label::new(desc).size(LabelSize::Small)),
                        )
                    })
                    // Blocked by
                    .when(!blocked_by.is_empty(), |this: gpui::Stateful<gpui::Div>| {
                        this.child(
                            v_flex()
                                .px_2()
                                .py_1()
                                .child(
                                    Label::new("Blocked by")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .children(
                                    blocked_by
                                        .iter()
                                        .map(|d| Self::render_dep_row(d, Color::Error, cx)),
                                ),
                        )
                    })
                    // Blocks
                    .when(!blocks.is_empty(), |this: gpui::Stateful<gpui::Div>| {
                        this.child(
                            v_flex()
                                .px_2()
                                .py_1()
                                .child(
                                    Label::new("Blocks")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .children(
                                    blocks
                                        .iter()
                                        .map(|d| Self::render_dep_row(d, Color::Warning, cx)),
                                ),
                        )
                    })
                    // Notes
                    .when(!notes.is_empty(), |this: gpui::Stateful<gpui::Div>| {
                        this.child(
                            v_flex()
                                .px_2()
                                .py_1()
                                .child(
                                    Label::new("Notes")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .children(notes.iter().map(|n| {
                                    v_flex()
                                        .px_1()
                                        .child(Label::new(n.title.clone()).size(LabelSize::Small))
                                        .when_some(n.content.clone(), |this: gpui::Div, c| {
                                            this.child(
                                                Label::new(c)
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted),
                                            )
                                        })
                                })),
                        )
                    })
                    // Latest handoff
                    .when_some(handoffs.first().cloned(), |this: gpui::Stateful<gpui::Div>, h| {
                        this.child(
                            v_flex()
                                .px_2()
                                .py_1()
                                .child(
                                    Label::new("Last Handoff")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .child(
                                    Label::new(format!("↳ {}", h.agent_name))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(Label::new(h.summary.clone()).size(LabelSize::Small))
                                .children(h.next_steps.iter().take(3).map(|s: &String| {
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            Label::new("·")
                                                .size(LabelSize::Small)
                                                .color(Color::Muted),
                                        )
                                        .child(Label::new(s.clone()).size(LabelSize::Small))
                                })),
                        )
                    })
                    // Recent activity
                    .when(!recent_activity.is_empty(), |this: gpui::Stateful<gpui::Div>| {
                        this.child(
                            v_flex()
                                .px_2()
                                .py_1()
                                .child(
                                    Label::new("Activity")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .children(recent_activity.iter().take(5).map(|e| {
                                    h_flex()
                                        .gap_1()
                                        .child(Label::new(e.actor.clone()).size(LabelSize::Small))
                                        .child(
                                            Label::new(e.action.clone())
                                                .size(LabelSize::Small)
                                                .color(Color::Muted),
                                        )
                                        .when_some(e.entity_name.clone(), |this: gpui::Div, n| {
                                            this.child(
                                                Label::new(n).size(LabelSize::Small).truncate(),
                                            )
                                        })
                                })),
                        )
                    }),
            )
    }

    fn render_agent_action_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .flex_none()
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .map(|this| {
                if let Some(name) = &self.agent_name {
                    let is_busy = self.checkin_task.is_some();
                    let is_in = self
                        .context
                        .as_ref()
                        .map(|c| {
                            c.active_agents
                                .iter()
                                .any(|a| &a.name == name && a.session_open)
                        })
                        .unwrap_or(false);
                    let name_label = name.clone();
                    this.child(
                        Label::new(name_label)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    )
                    .child(if is_busy {
                        Button::new("checkin-busy", if is_in { "Checking out..." } else { "Checking in..." })
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .disabled(true)
                            .into_any_element()
                    } else if is_in {
                        Button::new("checkout", "Check Out")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.run_checkout(cx)))
                            .into_any_element()
                    } else {
                        Button::new("checkin", "Check In")
                            .style(ButtonStyle::Filled)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.run_checkin(cx)))
                            .into_any_element()
                    })
                } else {
                    this.child(
                        Label::new("Set UH_AGENT_NAME to check in")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                }
            })
    }

    fn render_cost_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cost_expanded = self.cost_expanded;
        let has_api_key = self.prism_api_key.is_some();

        v_flex()
            .child(Self::render_section_header(
                "section-cost",
                "Cost",
                cost_expanded,
                cx.listener(|this, _, _, cx| {
                    this.cost_expanded = !this.cost_expanded;
                    this.serialize(cx);
                    cx.notify();
                }),
                cx,
            ))
            .when(cost_expanded, |this| {
                this.child(
                    v_flex().px_2().py_1().child(
                        Label::new(if has_api_key {
                            "Cost display coming soon."
                        } else {
                            "Set PRISM_API_KEY to enable cost tracking."
                        })
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    ),
                )
            })
    }
}

impl Render for TaskBoardPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_loading = self.is_loading;
        let error = self.error.clone();

        v_flex()
            .size_full()
            // Header bar
            .child(
                h_flex()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .h(px(32.))
                    .flex_none()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(Label::new("Task Board").size(LabelSize::Small))
                    .child(
                        IconButton::new("refresh", IconName::ArrowCircle)
                            .icon_size(ui::IconSize::Small)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.refresh(cx);
                            })),
                    ),
            )
            // Body
            .map(|container| {
                // Detail / loading states bypass the board entirely
                let loading_detail =
                    matches!(&self.view_state, ViewState::LoadingDetail { .. });
                let detail_ctx = if let ViewState::Detail(ctx) = &self.view_state {
                    Some(ctx.clone())
                } else {
                    None
                };

                if loading_detail {
                    return container.child(
                        v_flex().p_4().child(
                            Label::new("Loading task…")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    );
                }

                if let Some(ctx) = detail_ctx {
                    return container.child(self.render_detail_view(&ctx, cx));
                }

                // Board view
                if is_loading && self.context.is_none() {
                    return container.child(
                        v_flex().p_4().child(
                            Label::new("Loading task board...")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    );
                }

                if let Some(error_msg) = &error {
                    if self.context.is_none() {
                        let error_text = if error_msg.contains("uglyhat.json")
                            || error_msg.contains("not found")
                        {
                            "No .uglyhat.json found. Run `uh init` to set up.".to_owned()
                        } else {
                            format!("Error: {error_msg}")
                        };
                        return container.child(
                            v_flex().p_4().child(
                                Label::new(error_text)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                        );
                    }
                }

                let Some(ctx) = &self.context else {
                    return container.child(
                        v_flex().p_4().child(
                            Label::new("No tasks yet.")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    );
                };

                let active_tasks = ctx.active_tasks.clone();
                let next_tasks = self.next_tasks.clone();
                let status_counts = ctx.tasks_by_status.clone();
                let agents = ctx.active_agents.clone();
                let stale_tasks = ctx.stale_tasks.clone();
                let my_agent_name = self.agent_name.clone().unwrap_or_default();
                let agents_expanded = self.agents_expanded;
                let stale_expanded = self.stale_expanded;
                let active_expanded = self.active_expanded;
                let next_expanded = self.next_expanded;

                container
                    // Agents section
                    .child(Self::render_section_header(
                        "section-agents",
                        "Agents",
                        agents_expanded,
                        cx.listener(|this, _event, _window, cx| {
                            this.agents_expanded = !this.agents_expanded;
                            cx.notify();
                        }),
                        cx,
                    ))
                    .when(agents_expanded, |this| {
                        if agents.is_empty() {
                            this.child(
                                v_flex().px_2().py_1().child(
                                    Label::new("No agents registered.")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                            )
                        } else {
                            this.children(agents.iter().map(|a| Self::render_agent_row(a, &my_agent_name, cx)))
                        }
                    })
                    // Stale Tasks section (hidden when empty)
                    .when(!stale_tasks.is_empty(), |this| {
                        this.child(Self::render_section_header(
                            "section-stale",
                            "Stale Tasks",
                            stale_expanded,
                            cx.listener(|this, _event, _window, cx| {
                                this.stale_expanded = !this.stale_expanded;
                                cx.notify();
                            }),
                            cx,
                        ))
                        .when(stale_expanded, |this| {
                            this.children(stale_tasks.iter().map(|task| {
                                let task_id = task.id.clone();
                                let assignee = task
                                    .assignee
                                    .clone()
                                    .unwrap_or_else(|| "unassigned".to_owned());
                                h_flex()
                                    .w_full()
                                    .px_2()
                                    .py_1()
                                    .gap_2()
                                    .id(ElementId::Name(task_id.clone().into()))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(cx.theme().colors().element_hover))
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.open_task_detail(task_id.clone(), cx);
                                    }))
                                    .child(
                                        div()
                                            .w(px(6.))
                                            .h(px(6.))
                                            .rounded_full()
                                            .flex_none()
                                            .bg(Color::Warning.color(cx)),
                                    )
                                    .child(
                                        Label::new(task.name.clone())
                                            .size(LabelSize::Small)
                                            .truncate(),
                                    )
                                    .child(
                                        Label::new(assignee)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                            }))
                        })
                    })
                    // In Progress section
                    .child(Self::render_section_header(
                        "section-active",
                        "In Progress",
                        active_expanded,
                        cx.listener(|this, _event, _window, cx| {
                            this.active_expanded = !this.active_expanded;
                            cx.notify();
                        }),
                        cx,
                    ))
                    .when(active_expanded, |this| {
                        if active_tasks.is_empty() {
                            this.child(
                                v_flex().px_2().py_1().child(
                                    Label::new("No tasks in progress.")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                            )
                        } else {
                            this.children(active_tasks.iter().map(|task| {
                                let task_id = task.id.clone();
                                Self::render_task_row(task, cx)
                                    .id(ElementId::Name(task_id.clone().into()))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.open_task_detail(task_id.clone(), cx);
                                    }))
                            }))
                        }
                    })
                    // Up Next section
                    .child(Self::render_section_header(
                        "section-next",
                        "Up Next",
                        next_expanded,
                        cx.listener(|this, _event, _window, cx| {
                            this.next_expanded = !this.next_expanded;
                            cx.notify();
                        }),
                        cx,
                    ))
                    .when(next_expanded, |this| {
                        if next_tasks.is_empty() {
                            this.child(
                                v_flex().px_2().py_1().child(
                                    Label::new("No tasks in backlog.")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                            )
                        } else {
                            this.children(next_tasks.iter().map(|task| {
                                let task_id = task.id.clone();
                                Self::render_task_row(task, cx)
                                    .id(ElementId::Name(task_id.clone().into()))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.open_task_detail(task_id.clone(), cx);
                                    }))
                            }))
                        }
                    })
                    // Cost section
                    .child(self.render_cost_section(cx))
                    // Agent action bar
                    .child(self.render_agent_action_bar(cx))
                    // Summary footer
                    .when(!status_counts.is_empty(), |this| {
                        this.child(Self::render_summary_footer(&status_counts))
                    })
            })
    }
}

impl Focusable for TaskBoardPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<Event> for TaskBoardPanel {}
impl EventEmitter<PanelEvent> for TaskBoardPanel {}

impl Panel for TaskBoardPanel {
    fn persistent_name() -> &'static str {
        "TaskBoardPanel"
    }

    fn panel_key() -> &'static str {
        TASK_BOARD_PANEL_KEY
    }

    fn position(&self, _: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, _position: DockPosition, _: &mut Window, _cx: &mut Context<Self>) {
        // Position is hardcoded for v1
    }

    fn size(&self, _: &Window, _cx: &App) -> Pixels {
        self.width.unwrap_or(px(320.))
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
        Some(IconName::ListTodo)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Task Board")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        7
    }
}
