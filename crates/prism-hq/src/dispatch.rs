use gpui::{
    px, App, AppContext as _, Context, DismissEvent, EventEmitter, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, ParentElement, Render, Styled, Task, WeakEntity, Window, actions,
};
use ui::{h_flex, prelude::*, v_flex, Button, ButtonStyle, Color, Label, LabelSize};
use workspace::{ModalView, Workspace};

use crate::agent_spawner::spawn_agent_in_worktree;
use uglyhat_panel::UglyhatService;

actions!(prism_hq, [DispatchTask]);

#[derive(Debug, Clone, Copy, PartialEq)]
enum DispatchField {
    Task,
    ThreadName,
}

pub struct TaskDispatchModal {
    focus_handle: FocusHandle,
    task_input: String,
    thread_name_input: String,
    active_field: DispatchField,
    dispatching: bool,
    dispatched_thread: Option<String>,
    error: Option<String>,
    workspace: WeakEntity<Workspace>,
    dispatch_task: Option<Task<()>>,
}

impl EventEmitter<DismissEvent> for TaskDispatchModal {}
impl ModalView for TaskDispatchModal {}

impl Focusable for TaskDispatchModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TaskDispatchModal {
    pub fn new(workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            task_input: String::new(),
            thread_name_input: String::new(),
            active_field: DispatchField::Task,
            dispatching: false,
            dispatched_thread: None,
            error: None,
            workspace,
            dispatch_task: None,
        }
    }

    pub fn open(
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let workspace_weak = cx.weak_entity();
        workspace.toggle_modal(window, cx, move |_, cx| {
            Self::new(workspace_weak, cx)
        });
    }

    /// Derive thread name slug from task input.
    fn sync_thread_name(&mut self) {
        let slug: String = self
            .task_input
            .chars()
            .take(40)
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        // Collapse consecutive dashes
        let mut out = String::new();
        let mut prev_dash = false;
        for c in slug.chars() {
            if c == '-' {
                if !prev_dash {
                    out.push(c);
                }
                prev_dash = true;
            } else {
                out.push(c);
                prev_dash = false;
            }
        }
        self.thread_name_input = out.trim_matches('-').to_string();
    }

    fn dispatch(&mut self, cx: &mut Context<Self>) {
        if self.task_input.trim().is_empty() || self.dispatching {
            return;
        }

        self.dispatching = true;
        self.error = None;
        cx.notify();

        let task_input = self.task_input.trim().to_string();
        let thread_name = if self.thread_name_input.is_empty() {
            "untitled".to_string()
        } else {
            self.thread_name_input.clone()
        };
        self.dispatch_task = Some(cx.spawn(async move |this, cx| {
            // Extract handle and repo_root from the foreground context.
            let (handle, repo_root) = this
                .update(cx, |this, cx| {
                    let handle =
                        cx.try_global::<UglyhatService>().and_then(|svc| svc.handle());
                    let repo_root = this.workspace.upgrade().and_then(|ws| {
                        ws.read(cx)
                            .project()
                            .read(cx)
                            .visible_worktrees(cx)
                            .next()
                            .map(|wt| wt.read(cx).abs_path().to_path_buf())
                    });
                    (handle, repo_root)
                })
                .unwrap_or((None, None));

            let thread_name_bg = thread_name.clone();
            let task_input_bg = task_input.clone();

            // Create the uglyhat thread (block_on internally).
            let create_result: anyhow::Result<()> = cx
                .background_spawn(async move {
                    let handle = handle
                        .ok_or_else(|| anyhow::anyhow!("uglyhat not available"))?;
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

            // Spawn agent in worktree.
            let spawn_result = if let Some(repo_root) = repo_root {
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let agent_name = format!("agent-{millis}");
                spawn_agent_in_worktree(
                    task_input.clone(),
                    thread_name.clone(),
                    agent_name,
                    repo_root,
                    cx,
                )
                .await
            } else {
                // No workspace open — still succeeded in creating the thread.
                Ok(())
            };

            this.update(cx, |this, cx| {
                this.dispatching = false;
                this.dispatch_task = None;
                match spawn_result {
                    Ok(()) => {
                        this.dispatched_thread = Some(thread_name.clone());
                    }
                    Err(e) => {
                        // Thread was created; spawn failed. Report error but still show success.
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let task_text = self.task_input.clone();
        let thread_text = self.thread_name_input.clone();
        let dispatching = self.dispatching;
        let error = self.error.clone();

        // Success state: show dispatched thread name.
        if let Some(ref thread_name) = self.dispatched_thread.clone() {
            let thread_name = thread_name.clone();
            return v_flex()
                .key_context("DispatchModal")
                .track_focus(&self.focus_handle)
                .w(px(520.))
                .p_4()
                .gap_3()
                .child(Label::new("Agent Dispatched").size(LabelSize::Small))
                .child(
                    h_flex()
                        .gap_1()
                        .child(Label::new("Thread:").size(LabelSize::XSmall).color(Color::Muted))
                        .child(Label::new(thread_name).size(LabelSize::XSmall).color(Color::Accent)),
                )
                .when_some(error, |this, err| {
                    this.child(Label::new(err).size(LabelSize::XSmall).color(Color::Warning))
                })
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("close", "Close")
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                        ),
                )
                .into_any();
        }

        v_flex()
            .key_context("DispatchModal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let ks = &event.keystroke;
                if ks.key == "escape" {
                    cx.emit(DismissEvent);
                } else if ks.key == "backspace" {
                    match this.active_field {
                        DispatchField::Task => {
                            this.task_input.pop();
                            this.sync_thread_name();
                        }
                        DispatchField::ThreadName => {
                            this.thread_name_input.pop();
                        }
                    }
                    cx.notify();
                } else if ks.key == "tab" {
                    this.active_field = match this.active_field {
                        DispatchField::Task => DispatchField::ThreadName,
                        DispatchField::ThreadName => DispatchField::Task,
                    };
                    cx.notify();
                } else if !ks.modifiers.platform
                    && !ks.modifiers.control
                    && !ks.modifiers.alt
                {
                    if let Some(ch) = &ks.key_char {
                        match this.active_field {
                            DispatchField::Task => {
                                this.task_input.push_str(ch);
                                this.sync_thread_name();
                            }
                            DispatchField::ThreadName => {
                                this.thread_name_input.push_str(ch);
                            }
                        }
                        cx.notify();
                    }
                }
            }))
            .w(px(520.))
            .p_4()
            .gap_3()
            // Header
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
            // Task input area
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
                            .id("task-input")
                            .px_2()
                            .py_2()
                            .min_h(px(80.))
                            .border_1()
                            .border_color(if self.active_field == DispatchField::Task {
                                cx.theme().colors().border_focused
                            } else {
                                cx.theme().colors().border
                            })
                            .rounded_md()
                            .bg(cx.theme().colors().editor_background)
                            .cursor_text()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.active_field = DispatchField::Task;
                                cx.notify();
                            }))
                            .child(if task_text.is_empty() {
                                Label::new("Describe the task…")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted)
                                    .into_any_element()
                            } else {
                                Label::new(task_text)
                                    .size(LabelSize::Small)
                                    .into_any_element()
                            }),
                    ),
            )
            // Thread name row
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new("Thread:").size(LabelSize::XSmall).color(Color::Muted),
                    )
                    .child(
                        div()
                            .id("thread-name-input")
                            .flex_1()
                            .px_2()
                            .py_0p5()
                            .border_1()
                            .border_color(if self.active_field == DispatchField::ThreadName {
                                cx.theme().colors().border_focused
                            } else {
                                cx.theme().colors().border
                            })
                            .rounded_sm()
                            .cursor_text()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.active_field = DispatchField::ThreadName;
                                cx.notify();
                            }))
                            .child(if thread_text.is_empty() {
                                Label::new("auto")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .into_any_element()
                            } else {
                                Label::new(thread_text)
                                    .size(LabelSize::XSmall)
                                    .into_any_element()
                            }),
                    ),
            )
            .when_some(error, |this, err| {
                this.child(Label::new(err).size(LabelSize::XSmall).color(Color::Error))
            })
            // Action bar
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("dispatch", if dispatching { "Dispatching…" } else { "Dispatch" })
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
