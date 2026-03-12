use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Task, WeakEntity, Window, actions,
};
use prism_context::model::{
    AgentSession, Decision, Handoff, HandoffStatus, Memory, Thread, ThreadContext, ThreadStatus,
    WorkPackage, WorkPackageStatus,
};
use ui::{
    Button, ButtonStyle, Color, Icon, IconName, Label, LabelSize, TintColor, h_flex, prelude::*,
    v_flex,
};
use workspace::Workspace;
use workspace::item::{Item, ItemEvent};

use crate::hq_state::HqState;
use crate::thread_view::open_thread_view;
use crate::context_service::ContextService;

actions!(prism_hq, [OpenTaskBoard]);

#[derive(Default, Clone, Copy, PartialEq)]
enum BoardFilter {
    #[default]
    All,
    Backlog,
    InProgress,
    Review,
    Done,
}

impl BoardFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Backlog => "Backlog",
            Self::InProgress => "In Progress",
            Self::Review => "Review",
            Self::Done => "Done",
        }
    }
}

enum ViewState {
    Board,
    LoadingDetail { thread_name: String },
    Detail(Box<ThreadContext>),
}

pub struct TaskBoardItem {
    focus_handle: FocusHandle,
    hq_state: Entity<HqState>,
    _hq_subscription: Subscription,
    filter: BoardFilter,
    view_state: ViewState,
    detail_task: Option<Task<()>>,
    workspace: Option<WeakEntity<Workspace>>,
}

impl TaskBoardItem {
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
            _hq_subscription: subscription,
            filter: BoardFilter::All,
            view_state: ViewState::Board,
            detail_task: None,
            workspace,
        }
    }

    fn is_blocked(thread: &Thread, all_threads: &[Thread]) -> bool {
        // A thread is blocked if any of its dependencies is not yet archived.
        // Unknown deps (not in workspace) are treated as non-blocking (conservative).
        thread.depends_on.iter().any(|dep_id| {
            all_threads
                .iter()
                .find(|t| t.id == *dep_id)
                .map(|t| t.status != ThreadStatus::Archived)
                .unwrap_or(false)
        })
    }

    fn classify_wp(wp: &WorkPackage) -> BoardFilter {
        match wp.status {
            WorkPackageStatus::Draft | WorkPackageStatus::Planned => BoardFilter::Backlog,
            WorkPackageStatus::Ready | WorkPackageStatus::InProgress => BoardFilter::InProgress,
            WorkPackageStatus::Review => BoardFilter::Review,
            WorkPackageStatus::Done | WorkPackageStatus::Cancelled => BoardFilter::Done,
        }
    }

    fn classify(thread: &Thread, handoffs: &[Handoff], all_threads: &[Thread]) -> BoardFilter {
        if thread.status == ThreadStatus::Archived {
            return BoardFilter::Done;
        }
        if Self::is_blocked(thread, all_threads) {
            return BoardFilter::Backlog;
        }
        let handoff = handoffs.iter().find(|h| h.thread_id == Some(thread.id));
        match handoff {
            None => BoardFilter::Backlog,
            Some(h) => match h.status {
                HandoffStatus::Running | HandoffStatus::Accepted | HandoffStatus::Pending => {
                    BoardFilter::InProgress
                }
                HandoffStatus::Completed => BoardFilter::Review,
                HandoffStatus::Failed | HandoffStatus::Cancelled => BoardFilter::Backlog,
            },
        }
    }

    fn open_thread_detail(&mut self, thread_name: String, cx: &mut Context<Self>) {
        self.view_state = ViewState::LoadingDetail {
            thread_name: thread_name.clone(),
        };
        cx.notify();

        self.detail_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<ContextService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<ThreadContext> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("uglyhat not available"))?;
                    handle.recall_thread(&thread_name)
                })
                .await;

            this.update(cx, |this, cx| {
                this.view_state = match result {
                    Ok(ctx) => ViewState::Detail(Box::new(ctx)),
                    Err(_) => ViewState::Board,
                };
                cx.notify();
            })
            .ok();
        }));
    }
}

impl EventEmitter<ItemEvent> for TaskBoardItem {}

impl Focusable for TaskBoardItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for TaskBoardItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Task Board".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::ListTodo))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

impl Render for TaskBoardItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.view_state {
            ViewState::Board => {
                let state = self.hq_state.read(cx);
                let handoffs = state.handoffs.clone();
                let all_threads = state.threads.clone();
                let all_wps = state.work_packages.clone();

                let filter = self.filter;

                let filters = [
                    BoardFilter::All,
                    BoardFilter::Backlog,
                    BoardFilter::InProgress,
                    BoardFilter::Review,
                    BoardFilter::Done,
                ];

                let filter_row = h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .children(filters.into_iter().map(|f| {
                        let is_selected = filter == f;
                        Button::new(f.label(), f.label())
                            .style(if is_selected {
                                ButtonStyle::Tinted(TintColor::Accent)
                            } else {
                                ButtonStyle::Subtle
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.filter = f;
                                cx.notify();
                            }))
                    }));

                let filtered: Vec<(usize, Thread)> = all_threads
                    .iter()
                    .cloned()
                    .enumerate()
                    .filter(|(_, t)| {
                        filter == BoardFilter::All
                            || Self::classify(t, &handoffs, &all_threads) == filter
                    })
                    .collect();

                let cards: Vec<_> = filtered
                    .into_iter()
                    .map(|(ix, thread)| {
                        let name = thread.name.clone();
                        let desc = thread.description.clone();
                        let tags = thread.tags.clone();
                        let assigned = handoffs.iter().any(|h| h.thread_id == Some(thread.id));
                        let is_blocked = Self::is_blocked(&thread, &all_threads);
                        let cost = thread.cost_spent_usd;
                        let confidence = thread.confidence;
                        let thread_name_for_click = thread.name.clone();
                        let ws = self.workspace.clone();

                        v_flex()
                            .id(("thread-card", ix))
                            .w_full()
                            .p_2()
                            .gap_0p5()
                            .rounded_sm()
                            .bg(cx.theme().colors().element_background)
                            .cursor_pointer()
                            .hover(|s| s.bg(cx.theme().colors().element_hover))
                            .on_click(cx.listener(move |_, _, window, cx| {
                                if let Some(ws_ref) = ws.as_ref().and_then(|w| w.upgrade()) {
                                    ws_ref.update(cx, |workspace, cx| {
                                        open_thread_view(
                                            workspace,
                                            thread_name_for_click.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }))
                            .child(Label::new(name).size(LabelSize::Small))
                            .when(!desc.is_empty(), |this| {
                                let truncated = if desc.len() > 80 {
                                    format!("{}…", &desc[..80])
                                } else {
                                    desc.clone()
                                };
                                this.child(
                                    Label::new(truncated)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            })
                            .when(!tags.is_empty(), |this| {
                                this.child(h_flex().gap_0p5().children(tags.into_iter().map(
                                    |tag| {
                                        Label::new(tag).size(LabelSize::XSmall).color(Color::Accent)
                                    },
                                )))
                            })
                            .when(assigned, |this| {
                                this.child(
                                    Label::new("assigned")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Success),
                                )
                            })
                            .when(is_blocked, |this| {
                                this.child(
                                    Label::new("blocked")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Error),
                                )
                            })
                            .when(cost > 0.001, |this| {
                                this.child(
                                    Label::new(format!("${cost:.2}"))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Warning),
                                )
                            })
                            .when_some(confidence, |this, conf| {
                                let pct = (conf * 100.0).round() as u32;
                                let color = if pct >= 80 {
                                    Color::Success
                                } else if pct >= 50 {
                                    Color::Warning
                                } else {
                                    Color::Error
                                };
                                this.child(
                                    Label::new(format!("{pct}%"))
                                        .size(LabelSize::XSmall)
                                        .color(color),
                                )
                            })
                    })
                    .collect();

                // Work package cards classified by status
                let wp_cards: Vec<_> = all_wps
                    .iter()
                    .filter(|wp| filter == BoardFilter::All || Self::classify_wp(wp) == filter)
                    .enumerate()
                    .map(|(ix, wp)| {
                        let intent = wp.intent.clone();
                        let status_label = match wp.status {
                            WorkPackageStatus::Draft => "draft",
                            WorkPackageStatus::Planned => "planned",
                            WorkPackageStatus::Ready => "ready",
                            WorkPackageStatus::InProgress => "in progress",
                            WorkPackageStatus::Review => "review",
                            WorkPackageStatus::Done => "done",
                            WorkPackageStatus::Cancelled => "cancelled",
                        };
                        let status_color = match wp.status {
                            WorkPackageStatus::Done => Color::Success,
                            WorkPackageStatus::InProgress => Color::Accent,
                            WorkPackageStatus::Review => Color::Warning,
                            WorkPackageStatus::Ready => Color::Default,
                            _ => Color::Muted,
                        };
                        let agent = wp.assigned_agent.clone();
                        v_flex()
                            .id(("wp-card", ix))
                            .w_full()
                            .p_2()
                            .gap_0p5()
                            .rounded_sm()
                            .bg(cx.theme().colors().element_background)
                            .border_l_2()
                            .border_color(Color::Accent.color(cx))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Label::new("WP")
                                            .size(LabelSize::XSmall)
                                            .color(Color::Accent),
                                    )
                                    .child(
                                        Label::new(format!("#{}", wp.ordinal + 1))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .flex_1()
                                    .child(
                                        Label::new(status_label)
                                            .size(LabelSize::XSmall)
                                            .color(status_color),
                                    ),
                            )
                            .child(Label::new(intent).size(LabelSize::Small))
                            .when_some(agent, |this, a| {
                                this.child(
                                    Label::new(a).size(LabelSize::XSmall).color(Color::Accent),
                                )
                            })
                    })
                    .collect();

                let has_wps = !wp_cards.is_empty();
                let is_empty = cards.is_empty() && !has_wps;

                v_flex()
                    .key_context("TaskBoard")
                    .track_focus(&self.focus_handle)
                    .size_full()
                    .bg(cx.theme().colors().editor_background)
                    .child(filter_row)
                    .child(
                        v_flex()
                            .id("thread-cards")
                            .flex_1()
                            .overflow_y_scroll()
                            .px_2()
                            .py_1()
                            .gap_1()
                            .when(is_empty, |this| {
                                this.child(
                                    Label::new("No items in this column")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                            })
                            .children(wp_cards)
                            .when(!cards.is_empty() && has_wps, |this| {
                                // Separator between WPs and threads
                                this.child(
                                    h_flex().py_0p5().child(
                                        Label::new("— threads —")
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    ),
                                )
                            })
                            .children(cards),
                    )
                    .into_any()
            }
            ViewState::LoadingDetail { thread_name } => {
                let name = thread_name.clone();
                v_flex()
                    .key_context("TaskBoard")
                    .track_focus(&self.focus_handle)
                    .size_full()
                    .bg(cx.theme().colors().editor_background)
                    .child(
                        Label::new(format!("Loading {}…", name))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any()
            }
            ViewState::Detail(ctx) => {
                let thread_name: SharedString = ctx.thread.name.clone().into();
                let thread_desc = ctx.thread.description.clone();
                let memories = ctx.memories.clone();
                let decisions = ctx.decisions.clone();
                let sessions = ctx.recent_sessions.clone();

                v_flex()
                    .key_context("TaskBoard")
                    .track_focus(&self.focus_handle)
                    .size_full()
                    .bg(cx.theme().colors().editor_background)
                    .child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .h(px(32.))
                            .border_b_1()
                            .border_color(cx.theme().colors().border)
                            .gap_1()
                            .child(ui::IconButton::new("back", IconName::ArrowLeft).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.view_state = ViewState::Board;
                                    cx.notify();
                                }),
                            ))
                            .child(Label::new(thread_name).size(LabelSize::Small)),
                    )
                    .child(
                        v_flex()
                            .id("detail-content")
                            .flex_1()
                            .overflow_y_scroll()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .when(!thread_desc.is_empty(), |this| {
                                this.child(
                                    Label::new(thread_desc)
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                            })
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        Label::new("MEMORIES")
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .when(memories.is_empty(), |this| {
                                        this.child(
                                            Label::new("No memories")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                    })
                                    .children(memories.into_iter().map(|m: Memory| {
                                        v_flex()
                                            .gap_0p5()
                                            .child(
                                                Label::new(m.key.clone())
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Accent),
                                            )
                                            .child(Label::new(m.value).size(LabelSize::XSmall))
                                    })),
                            )
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        Label::new("DECISIONS")
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .when(decisions.is_empty(), |this| {
                                        this.child(
                                            Label::new("No decisions")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                    })
                                    .children(decisions.into_iter().map(|d: Decision| {
                                        v_flex()
                                            .gap_0p5()
                                            .child(
                                                Label::new(d.title.clone())
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Accent),
                                            )
                                            .when(!d.content.is_empty(), |this| {
                                                this.child(
                                                    Label::new(d.content.clone())
                                                        .size(LabelSize::XSmall),
                                                )
                                            })
                                    })),
                            )
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        Label::new("RECENT SESSIONS")
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .when(sessions.is_empty(), |this| {
                                        this.child(
                                            Label::new("No sessions")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                    })
                                    .children(sessions.into_iter().map(|s: AgentSession| {
                                        let summary = if s.summary.is_empty() {
                                            "no summary".to_string()
                                        } else {
                                            s.summary.clone()
                                        };
                                        v_flex()
                                            .gap_0p5()
                                            .child(
                                                Label::new(
                                                    s.started_at
                                                        .format("%Y-%m-%d %H:%M")
                                                        .to_string(),
                                                )
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                            )
                                            .child(Label::new(summary).size(LabelSize::XSmall))
                                    })),
                            ),
                    )
                    .into_any()
            }
        }
    }
}

/// Open or activate the Task Board in the active workspace.
pub fn open_task_board(
    workspace: &mut workspace::Workspace,
    hq_state: Entity<HqState>,
    window: &mut Window,
    cx: &mut Context<workspace::Workspace>,
) {
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<TaskBoardItem>());

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let weak_workspace = cx.weak_entity();
        let item = cx.new(|cx: &mut Context<TaskBoardItem>| {
            TaskBoardItem::new(hq_state, Some(weak_workspace), window, cx)
        });
        workspace.add_item_to_center(Box::new(item), window, cx);
    }
}
