/// Multi-file diff review panel for agent-proposed edits.
///
/// When the agent proposes `edit_file` tool calls across multiple files in a
/// single turn, this component collects all of them and shows a unified review
/// UI before any edits are applied.  The user can bulk accept/reject or
/// toggle individual files.
///
/// # Wiring status (TODO)
///
/// The data model and rendering are implemented here.  Full end-to-end wiring
/// into the agent tool execution pipeline in `thread.rs` is left for a follow-
/// up task.  To complete the wiring:
///
/// 1. Add a `pending_multi_file_review: Option<Entity<MultiFileDiffReview>>`
///    field to the `Thread` struct in `crates/agent/src/thread.rs`.
///
/// 2. In `Thread::handle_completion_event`, when an `EditFileTool` /
///    `StreamingEditFileTool` call completes, push a `PendingEdit` onto the
///    review entity instead of immediately applying the buffer edit.
///
/// 3. Emit a new `ThreadEvent::MultiFileEditPending(Entity<MultiFileDiffReview>)`
///    variant so the `AgentPanel` can open this pane.
///
/// 4. Wire `AgentPanel` to subscribe to that event and call
///    `MultiFileDiffReview::deploy(review, workspace, window, cx)`.
///
/// 5. On "Accept All" call `PendingEdit::apply` for each edit.
///    On "Reject All" just drop the entity.
use std::path::PathBuf;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Task, WeakEntity,
    Window, prelude::*,
};
use project::Project;
use ui::{ElevationIndex, prelude::*};
use workspace::{
    Item, Workspace,
    item::{ItemEvent, TabContentParams},
};

/// A single file edit proposed by the agent — original content vs. proposed content.
#[derive(Clone)]
pub struct PendingEdit {
    /// Absolute path of the file to be edited.
    pub file_path: PathBuf,
    /// Original file content before the edit.
    pub original: String,
    /// Agent-proposed replacement content.
    pub proposed: String,
    /// Whether the user has toggled this individual edit on (true = accept).
    pub accepted: bool,
}

impl PendingEdit {
    pub fn new(file_path: PathBuf, original: String, proposed: String) -> Self {
        Self {
            file_path,
            original,
            proposed,
            accepted: true,
        }
    }

    /// Apply this edit to the project buffer.
    ///
    /// TODO: Implement buffer lookup + write using `project::Project::open_path`
    /// and `Buffer::edit`.  Currently a no-op stub.
    pub fn apply(&self, _project: &Entity<Project>) -> Task<anyhow::Result<()>> {
        log::info!(
            "MultiFileDiffReview: applying edit to {}",
            self.file_path.display()
        );
        // TODO: open the buffer from project, apply the proposed text, save.
        Task::ready(Ok(()))
    }
}

pub enum MultiFileDiffReviewEvent {
    Accepted,
    Rejected,
}

impl EventEmitter<MultiFileDiffReviewEvent> for MultiFileDiffReview {}

pub struct MultiFileDiffReview {
    pending_edits: Vec<PendingEdit>,
    project: Entity<Project>,
    focus_handle: FocusHandle,
    _apply_task: Option<Task<()>>,
}

impl MultiFileDiffReview {
    /// Open (or re-use) a `MultiFileDiffReview` pane in the workspace.
    pub fn deploy(
        pending_edits: Vec<PendingEdit>,
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut App,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update(cx, |workspace, cx| {
            let review = cx.new(|cx| Self::new(pending_edits, project, cx));
            workspace.add_item_to_center(Box::new(review.clone()), window, cx);
            review
        })
    }

    pub fn new(
        pending_edits: Vec<PendingEdit>,
        project: Entity<Project>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            pending_edits,
            project,
            focus_handle: cx.focus_handle(),
            _apply_task: None,
        }
    }

    pub fn pending_edits(&self) -> &[PendingEdit] {
        &self.pending_edits
    }

    /// Toggle the accepted state for a single file by index.
    pub fn toggle_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(edit) = self.pending_edits.get_mut(index) {
            edit.accepted = !edit.accepted;
            cx.notify();
        }
    }

    /// Mark all pending edits as accepted.
    pub fn accept_all(&mut self, cx: &mut Context<Self>) {
        for edit in &mut self.pending_edits {
            edit.accepted = true;
        }
        let project = self.project.clone();
        let edits: Vec<_> = self.pending_edits.clone();
        self._apply_task = Some(cx.spawn(async move |this, cx| {
            for edit in edits {
                if edit.accepted {
                    let task = edit.apply(&project);
                    if let Err(err) = task.await {
                        log::error!(
                            "MultiFileDiffReview: failed to apply edit to {}: {err}",
                            edit.file_path.display()
                        );
                    }
                }
            }
            this.update(cx, |_, cx| cx.emit(MultiFileDiffReviewEvent::Accepted))
                .ok();
        }));
    }

    /// Discard all pending edits.
    pub fn reject_all(&mut self, cx: &mut Context<Self>) {
        self.pending_edits.clear();
        cx.emit(MultiFileDiffReviewEvent::Rejected);
        cx.notify();
    }
}

impl Focusable for MultiFileDiffReview {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MultiFileDiffReview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let file_count = self.pending_edits.len();
        let accepted_count = self.pending_edits.iter().filter(|e| e.accepted).count();

        let file_tabs = v_flex()
            .gap_1()
            .children(
                self.pending_edits
                    .iter()
                    .enumerate()
                    .map(|(index, edit)| {
                        let file_name = edit
                            .file_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let full_path = edit.file_path.display().to_string();
                        let accepted = edit.accepted;

                        h_flex()
                            .p_1()
                            .gap_2()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .bg(cx.theme().colors().surface_background)
                            .child(
                                Icon::new(if accepted {
                                    IconName::Check
                                } else {
                                    IconName::XCircle
                                })
                                .color(if accepted {
                                    Color::Success
                                } else {
                                    Color::Muted
                                })
                                .size(IconSize::Small),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_x_hidden()
                                    .text_ellipsis()
                                    .child(
                                        Label::new(file_name)
                                            .size(LabelSize::Small),
                                    )
                                    .child(
                                        Label::new(full_path)
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    ),
                            )
                            .child(
                                Button::new(
                                    ("toggle", index),
                                    if accepted { "Reject" } else { "Accept" },
                                )
                                .label_size(LabelSize::XSmall)
                                .layer(ElevationIndex::ModalSurface)
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.toggle_file(index, cx);
                                })),
                            )
                    }),
            );

        v_flex()
            .size_full()
            .p_2()
            .gap_2()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        Label::new(format!(
                            "Agent proposed edits to {file_count} file{} — {accepted_count} selected",
                            if file_count == 1 { "" } else { "s" }
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("reject-all", "Reject All")
                                    .label_size(LabelSize::Small)
                                    .icon(IconName::XCircle)
                                    .icon_size(IconSize::Small)
                                    .icon_position(IconPosition::Start)
                                    .layer(ElevationIndex::ModalSurface)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.reject_all(cx);
                                    })),
                            )
                            .child(
                                Button::new("accept-all", "Accept All")
                                    .label_size(LabelSize::Small)
                                    .icon(IconName::Check)
                                    .icon_size(IconSize::Small)
                                    .icon_position(IconPosition::Start)
                                    .layer(ElevationIndex::ModalSurface)
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.accept_all(cx);
                                    })),
                            ),
                    ),
            )
            .child(file_tabs)
            .when(self.pending_edits.is_empty(), |this| {
                this.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Label::new("No pending edits.").color(Color::Muted)),
                )
            })
    }
}

impl EventEmitter<ItemEvent> for MultiFileDiffReview {}

impl Item for MultiFileDiffReview {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        let count = self.pending_edits.len();
        format!("Review Edits ({count})").into()
    }

    fn tab_content(&self, params: TabContentParams, _window: &Window, _cx: &App) -> AnyElement {
        let count = self.pending_edits.len();
        Label::new(format!("Review Edits ({count})"))
            .color(if params.selected {
                Color::Default
            } else {
                Color::Muted
            })
            .into_any_element()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Pencil))
    }

    fn to_item_events(event: &ItemEvent, f: &mut dyn FnMut(ItemEvent)) {
        f(event.clone())
    }
}
