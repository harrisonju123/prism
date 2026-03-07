use crate::types::{StatusCount, TaskSummary, WorkspaceContext};
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
    Color, Icon, IconButton, IconName, Label, LabelSize, Tooltip, h_flex, prelude::*, v_flex,
};
use util::{ResultExt, TryFutureExt};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

const TASK_BOARD_PANEL_KEY: &str = "TaskBoardPanel";
const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

actions!(
    uglyhat_panel,
    [
        /// Toggles the task board panel.
        Toggle,
        /// Toggles focus on the task board panel.
        ToggleFocus
    ]
);

pub struct TaskBoardPanel {
    focus_handle: FocusHandle,
    width: Option<Pixels>,
    active: bool,
    context: Option<WorkspaceContext>,
    next_tasks: Vec<TaskSummary>,
    is_loading: bool,
    error: Option<String>,
    active_expanded: bool,
    next_expanded: bool,
    refresh_task: Option<Task<()>>,
    pending_serialization: Task<Option<()>>,
    _auto_refresh: Task<()>,
}

#[derive(Serialize, Deserialize)]
struct SerializedTaskBoardPanel {
    width: Option<Pixels>,
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
                active_expanded: true,
                next_expanded: true,
                refresh_task: None,
                pending_serialization: Task::ready(None),
                _auto_refresh: Task::ready(()),
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
                        TASK_BOARD_PANEL_KEY.into(),
                        serde_json::to_string(&SerializedTaskBoardPanel { width })?,
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
                    let output = std::process::Command::new("uh").arg("context").output()?;
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
                    let output = std::process::Command::new("uh").arg("next").output()?;
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

    fn priority_color(priority: &str) -> Color {
        match priority {
            "critical" => Color::Error,
            "high" => Color::Warning,
            "medium" => Color::Accent,
            _ => Color::Muted,
        }
    }

    fn render_task_row(task: &TaskSummary, cx: &App) -> impl IntoElement {
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
                let active_expanded = self.active_expanded;
                let next_expanded = self.next_expanded;

                container
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
                            this.children(
                                active_tasks
                                    .iter()
                                    .map(|task| Self::render_task_row(task, cx)),
                            )
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
                            this.children(
                                next_tasks
                                    .iter()
                                    .map(|task| Self::render_task_row(task, cx)),
                            )
                        }
                    })
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
