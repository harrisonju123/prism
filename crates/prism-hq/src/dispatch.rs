use editor::Editor;
use gpui::{
    App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, Focusable, IntoElement,
    ParentElement, Render, Styled, Task, WeakEntity, Window, actions, px,
};
use ui::{Button, ButtonStyle, Color, Label, LabelSize, h_flex, prelude::*, v_flex};
use workspace::{ModalView, Workspace};

use crate::agent_spawner::spawn_agent_in_worktree;
use crate::agent_view::open_agent_view;
use crate::context_service::ContextService;

actions!(prism_hq, [DispatchTask]);

/// Converts a string to a URL-safe slug: lowercase alphanumeric, dashes as separators.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in text.chars().take(40) {
        if c.is_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

pub struct TaskDispatchModal {
    task_editor: Entity<Editor>,
    thread_name_editor: Entity<Editor>,
    dispatching: bool,
    dispatched_thread: Option<String>,
    dispatched_agent: Option<String>,
    error: Option<String>,
    workspace: WeakEntity<Workspace>,
    dispatch_task: Option<Task<()>>,
}

impl EventEmitter<DismissEvent> for TaskDispatchModal {}
impl ModalView for TaskDispatchModal {
    fn fade_out_background(&self) -> bool {
        true
    }
}

impl Focusable for TaskDispatchModal {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.task_editor.focus_handle(cx)
    }
}

impl TaskDispatchModal {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let task_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Describe the task…", window, cx);
            editor
        });
        let thread_name_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("auto", window, cx);
            editor
        });
        Self {
            task_editor,
            thread_name_editor,
            dispatching: false,
            dispatched_thread: None,
            dispatched_agent: None,
            error: None,
            workspace,
            dispatch_task: None,
        }
    }

    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        let workspace_weak = cx.weak_entity();
        workspace.toggle_modal(window, cx, move |window, cx| {
            Self::new(workspace_weak, window, cx)
        });
    }

    fn confirm(&mut self, _: &menu::Confirm, _window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(cx);
    }

    fn cancel(&mut self, _: &menu::Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn dispatch(&mut self, cx: &mut Context<Self>) {
        let task_text = self.task_editor.read(cx).text(cx);
        if task_text.trim().is_empty() || self.dispatching {
            return;
        }

        self.dispatching = true;
        self.error = None;
        cx.notify();

        let task_input = task_text.trim().to_string();
        let thread_name_raw = self.thread_name_editor.read(cx).text(cx);
        let thread_name = if thread_name_raw.trim().is_empty() {
            let slug = slugify(&task_input);
            if slug.is_empty() {
                "untitled".to_string()
            } else {
                slug
            }
        } else {
            thread_name_raw.trim().to_string()
        };

        self.dispatch_task = Some(cx.spawn(async move |this, cx| {
            let (mut handle, repo_root) = this
                .update(cx, |this, cx| {
                    let handle = cx
                        .try_global::<ContextService>()
                        .and_then(|svc| svc.handle());
                    let repo_root = this.workspace.upgrade().and_then(|ws| {
                        ws.read(cx)
                            .project()
                            .read(cx)
                            .visible_worktrees(cx)
                            .next()
                            .map(|wt| wt.read(cx).abs_path().to_path_buf())
                    });
                    // If the handle is missing (global never set or store open failed),
                    // attempt on-demand init so dispatch works even when startup init lost the race.
                    if handle.is_none() {
                        if let Some(ref root) = repo_root {
                            let _ = ContextService::init(root, cx);
                        }
                    }
                    let handle = cx
                        .try_global::<ContextService>()
                        .and_then(|svc| svc.handle());
                    (handle, repo_root)
                })
                .unwrap_or((None, None));

            // Store open is async — poll briefly for up to ~2s before giving up.
            if handle.is_none() {
                for _ in 0..20 {
                    cx.background_spawn(async {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await
                    })
                    .await;
                    handle = this
                        .update(cx, |_, cx| {
                            cx.try_global::<ContextService>().and_then(|svc| svc.handle())
                        })
                        .unwrap_or(None);
                    if handle.is_some() {
                        break;
                    }
                }
            }

            let thread_name_bg = thread_name.clone();
            let task_input_bg = task_input.clone();

            let create_result: anyhow::Result<()> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("context service not available"))?;
                    handle.create_thread(&thread_name_bg, &task_input_bg, vec![])?;
                    anyhow::Ok(())
                })
                .await;

            if let Err(e) = create_result {
                this.update(cx, |this, cx| {
                    this.dispatching = false;
                    this.error = Some(e.to_string());
                    cx.notify();
                })
                .ok();
                return;
            }

            let (spawn_result, spawned_agent) = if let Some(repo_root) = repo_root {
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let agent_name = format!("agent-{millis}");
                let result = spawn_agent_in_worktree(agent_name.clone(), repo_root, cx).await;
                (result, Some(agent_name))
            } else {
                (Ok(()), None)
            };

            this.update(cx, |this, cx| {
                this.dispatching = false;
                this.dispatch_task = None;
                match (spawn_result, spawned_agent) {
                    (Ok(()), Some(agent_name)) => {
                        // Set both so the success screen shows while navigation fires.
                        this.dispatched_agent = Some(agent_name);
                        this.dispatched_thread = Some(thread_name.clone());
                    }
                    (Ok(()), None) => {
                        // Thread created but no workspace — show static success screen.
                        this.dispatched_thread = Some(thread_name.clone());
                    }
                    (Err(e), _) => {
                        this.dispatched_thread = Some(thread_name.clone());
                        this.error = Some(format!("Thread created, but spawn failed: {e}"));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for TaskDispatchModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dispatching = self.dispatching;
        let error = self.error.clone();

        // On successful agent spawn, fire navigation and let the success screen show
        // while the async dismiss propagates.
        if let Some(agent_name) = self.dispatched_agent.take() {
            let workspace = self.workspace.clone();
            cx.spawn_in(window, async move |this, cx| {
                if let Some(ws) = workspace.upgrade() {
                    ws.update_in(cx, |workspace, window, cx| {
                        open_agent_view(workspace, agent_name, window, cx);
                    })
                    .ok();
                }
                this.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
            })
            .detach();
        }

        if let Some(thread_name) = self.dispatched_thread.clone() {
            return v_flex()
                .elevation_3(cx)
                .overflow_hidden()
                .key_context("DispatchModal")
                .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
                .on_action(cx.listener(|_, _: &menu::Confirm, _, cx| cx.emit(DismissEvent)))
                .track_focus(&self.task_editor.focus_handle(cx))
                .w(px(520.))
                .p_4()
                .gap_3()
                .child(Label::new("Agent Dispatched").size(LabelSize::Small))
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Label::new("Thread:")
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new(thread_name)
                                .size(LabelSize::XSmall)
                                .color(Color::Accent),
                        ),
                )
                .when_some(error, |this, err| {
                    this.child(
                        Label::new(err)
                            .size(LabelSize::XSmall)
                            .color(Color::Warning),
                    )
                })
                .child(
                    h_flex().gap_2().child(
                        Button::new("close", "Close")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
                )
                .into_any();
        }

        v_flex()
            .elevation_3(cx)
            .overflow_hidden()
            .key_context("DispatchModal")
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .w(px(520.))
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(Label::new("Dispatch Agent Task").size(LabelSize::Small))
                    .child(
                        Label::new("cmd-shift-d")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        Label::new("What should the agent work on?")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_2()
                            .min_h(px(80.))
                            .border_1()
                            .border_color(cx.theme().colors().border_focused)
                            .rounded_md()
                            .bg(cx.theme().colors().editor_background)
                            .child(self.task_editor.clone()),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new("Thread:")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_0p5()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .rounded_sm()
                            .child(self.thread_name_editor.clone()),
                    ),
            )
            .when_some(error, |this, err| {
                this.child(Label::new(err).size(LabelSize::XSmall).color(Color::Error))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(
                            "dispatch",
                            if dispatching {
                                "Dispatching…"
                            } else {
                                "Dispatch"
                            },
                        )
                        .style(ButtonStyle::Filled)
                        .label_size(LabelSize::Small)
                        .disabled(dispatching)
                        .on_click(cx.listener(|this, _, _, cx| this.dispatch(cx))),
                    )
                    .child(
                        Button::new("cancel", "Cancel")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
            )
            .into_any()
    }
}
