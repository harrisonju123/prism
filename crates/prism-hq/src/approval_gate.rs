use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    ParentElement, Render, Styled, Window, px,
};
use ui::{Button, ButtonStyle, Color, Label, LabelSize, h_flex, prelude::*, v_flex};
use workspace::{ModalView, Workspace};

/// Result of the human approval gate.
#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    Approve,
    RequestChanges { message: String },
    Reject,
}

pub struct ApprovalGate {
    focus_handle: FocusHandle,
    task_name: String,
    task_description: String,
    diff_preview: String,
    session_cost_usd: Option<f64>,
    test_summary: Option<String>,
    worktree_branch: String,
    decision: Option<ApprovalDecision>,
    request_changes_text: String,
    show_changes_composer: bool,
    error: Option<String>,
    callback: Option<Box<dyn FnOnce(ApprovalDecision) + Send + 'static>>,
}

pub enum Event {
    Dismissed(Option<ApprovalDecision>),
}

impl EventEmitter<Event> for ApprovalGate {}
impl EventEmitter<DismissEvent> for ApprovalGate {}

impl ModalView for ApprovalGate {}

impl Focusable for ApprovalGate {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ApprovalGate {
    pub fn new(
        task_name: impl Into<String>,
        task_description: impl Into<String>,
        worktree_branch: impl Into<String>,
        diff_preview: impl Into<String>,
        session_cost_usd: Option<f64>,
        test_summary: Option<String>,
        callback: impl FnOnce(ApprovalDecision) + Send + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            task_name: task_name.into(),
            task_description: task_description.into(),
            diff_preview: diff_preview.into(),
            session_cost_usd,
            test_summary,
            worktree_branch: worktree_branch.into(),
            decision: None,
            request_changes_text: String::new(),
            show_changes_composer: false,
            error: None,
            callback: Some(Box::new(callback)),
        }
    }

    pub fn open(
        task_name: impl Into<String>,
        task_description: impl Into<String>,
        worktree_branch: impl Into<String>,
        diff_preview: impl Into<String>,
        session_cost_usd: Option<f64>,
        test_summary: Option<String>,
        callback: impl FnOnce(ApprovalDecision) + Send + 'static,
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let task_name = task_name.into();
        let task_description = task_description.into();
        let worktree_branch = worktree_branch.into();
        let diff_preview = diff_preview.into();

        workspace.toggle_modal(window, cx, move |_window, cx| {
            Self::new(
                task_name,
                task_description,
                worktree_branch,
                diff_preview,
                session_cost_usd,
                test_summary,
                callback,
                cx,
            )
        });
    }

    fn approve(&mut self, cx: &mut Context<Self>) {
        if let Some(callback) = self.callback.take() {
            callback(ApprovalDecision::Approve);
        }
        cx.emit(Event::Dismissed(Some(ApprovalDecision::Approve)));
        cx.emit(DismissEvent);
    }

    fn request_changes(&mut self, cx: &mut Context<Self>) {
        let message = self.request_changes_text.trim().to_string();
        if message.is_empty() {
            self.show_changes_composer = true;
            cx.notify();
            return;
        }
        if let Some(callback) = self.callback.take() {
            callback(ApprovalDecision::RequestChanges {
                message: message.clone(),
            });
        }
        cx.emit(Event::Dismissed(Some(ApprovalDecision::RequestChanges {
            message,
        })));
        cx.emit(DismissEvent);
    }

    fn reject(&mut self, cx: &mut Context<Self>) {
        if let Some(callback) = self.callback.take() {
            callback(ApprovalDecision::Reject);
        }
        cx.emit(Event::Dismissed(Some(ApprovalDecision::Reject)));
        cx.emit(DismissEvent);
    }

    fn render_diff_section(&self, cx: &App) -> impl IntoElement {
        let lines: Vec<&str> = self.diff_preview.lines().take(30).collect();
        let truncated = self.diff_preview.lines().count() > 30;

        v_flex()
            .w_full()
            .gap_0p5()
            .child(
                Label::new("Diff Preview")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                v_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .bg(cx.theme().colors().surface_background)
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .rounded_md()
                    .max_h(px(200.))
                    .overflow_hidden()
                    .children(lines.iter().map(|line| {
                        let color = if line.starts_with('+') {
                            Color::Success
                        } else if line.starts_with('-') {
                            Color::Error
                        } else if line.starts_with("@@") {
                            Color::Accent
                        } else {
                            Color::Muted
                        };
                        Label::new(line.to_string())
                            .size(LabelSize::XSmall)
                            .color(color)
                    }))
                    .when(truncated, |this| {
                        this.child(
                            Label::new("… (truncated)")
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    }),
            )
    }
}

impl Render for ApprovalGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let task_name = self.task_name.clone();
        let task_desc = self.task_description.clone();
        let branch = self.worktree_branch.clone();
        let cost_label = self
            .session_cost_usd
            .map(|c| format!("${:.4}", c))
            .unwrap_or_else(|| "unknown".into());
        let test_summary = self.test_summary.clone();
        let show_composer = self.show_changes_composer;
        let changes_text = self.request_changes_text.clone();

        v_flex()
            .key_context("ApprovalGate")
            .track_focus(&self.focus_handle)
            .when(show_composer, |this| {
                this.on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    let ks = &event.keystroke;
                    if ks.key == "backspace" {
                        this.request_changes_text.pop();
                        cx.notify();
                    } else if !ks.modifiers.platform && !ks.modifiers.control {
                        if let Some(ch) = &ks.key_char {
                            this.request_changes_text.push_str(ch);
                            cx.notify();
                        }
                    }
                }))
            })
            .w(px(560.))
            .max_h(px(640.))
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(Label::new("Merge Approval Gate").size(LabelSize::Small))
                    .child(
                        Label::new(format!("branch: {branch}"))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .child(Label::new(task_name).size(LabelSize::Small))
                    .when(!task_desc.is_empty(), |this| {
                        this.child(
                            Label::new(task_desc)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Label::new("Session cost:")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(Label::new(cost_label).size(LabelSize::Small)),
                    )
                    .when_some(test_summary, |this, ts| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Label::new("Tests:")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(Label::new(ts).size(LabelSize::Small)),
                        )
                    }),
            )
            .child(
                v_flex()
                    .id("diff-scroll")
                    .overflow_y_scroll()
                    .child(self.render_diff_section(cx)),
            )
            .when(show_composer, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            Label::new("Describe the changes needed:")
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
                                .min_h(px(60.))
                                .child(
                                    Label::new(if changes_text.is_empty() {
                                        "What should the agent change?".to_string()
                                    } else {
                                        changes_text
                                    })
                                    .size(LabelSize::Small)
                                    .color(
                                        if self.request_changes_text.is_empty() {
                                            Color::Muted
                                        } else {
                                            Color::Default
                                        },
                                    ),
                                ),
                        ),
                )
            })
            .when_some(self.error.clone(), |this, err| {
                this.child(Label::new(err).size(LabelSize::Small).color(Color::Error))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("approve", "Approve & Merge")
                            .style(ButtonStyle::Filled)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.approve(cx))),
                    )
                    .child(
                        Button::new("request-changes", "Request Changes")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.request_changes(cx))),
                    )
                    .child(
                        Button::new("reject", "Reject")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.reject(cx))),
                    ),
            )
    }
}
