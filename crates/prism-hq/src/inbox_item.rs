use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Task, WeakEntity, Window, actions,
};
use prism_context::model::{InboxEntry, InboxEntryType, InboxSeverity};
use ui::{
    Button, ButtonStyle, Color, Icon, IconName, Label, LabelSize, TintColor, h_flex, prelude::*,
    v_flex,
};
use workspace::Workspace;
use workspace::item::{Item, ItemEvent};

use crate::approval_gate::{ApprovalDecision, ApprovalGate};
use crate::context_service::{ContextHandle, ContextService};
use crate::hq_state::HqState;
use crate::thread_view::open_thread_view;

actions!(prism_hq, [OpenInbox]);

/// Filter for the inbox feed.
#[derive(Default, Clone, Copy, PartialEq)]
enum InboxFilter {
    #[default]
    All,
    Unread,
    Approval,
    Blocked,
    Suggestion,
}

impl InboxFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Unread => "Unread",
            Self::Approval => "Approval",
            Self::Blocked => "Blocked",
            Self::Suggestion => "Suggestion",
        }
    }

    fn matches(self, entry: &InboxEntry) -> bool {
        match self {
            Self::All => true,
            Self::Unread => !entry.read,
            Self::Approval => entry.entry_type == InboxEntryType::Approval,
            Self::Blocked => entry.entry_type == InboxEntryType::Blocked,
            Self::Suggestion => {
                matches!(
                    entry.entry_type,
                    InboxEntryType::Suggestion | InboxEntryType::Risk
                )
            }
        }
    }
}

pub struct InboxItem {
    focus_handle: FocusHandle,
    hq_state: Entity<HqState>,
    _hq_subscription: Subscription,
    filter: InboxFilter,
    /// Single in-flight store operation (dismiss or mark-read). Dropping cancels it.
    pending_op: Option<Task<()>>,
    workspace: Option<WeakEntity<Workspace>>,
}

impl InboxItem {
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
            filter: InboxFilter::All,
            pending_op: None,
            workspace,
        }
    }

    fn dismiss_entry(&mut self, id: uuid::Uuid, cx: &mut Context<Self>) {
        self.pending_op = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<ContextService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            if let Some(handle) = handle {
                let _ = cx
                    .background_spawn(async move { handle.dismiss_inbox_entry(id) })
                    .await;
            }

            this.update(cx, |_, cx| cx.notify()).ok();
        }));
    }

    fn mark_read(&mut self, id: uuid::Uuid, cx: &mut Context<Self>) {
        self.pending_op = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<ContextService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            if let Some(handle) = handle {
                let _ = cx
                    .background_spawn(async move { handle.mark_inbox_read(id) })
                    .await;
            }

            this.update(cx, |_, cx| cx.notify()).ok();
        }));
    }

    fn open_approval_gate(&mut self, entry: &InboxEntry, window: &mut Window, cx: &mut Context<Self>) {
        use prism_context::model::AgentState;
        use serde_json::json;

        let handle: Option<ContextHandle> = cx
            .try_global::<ContextService>()
            .and_then(|svc| svc.handle());
        let Some(handle) = handle else { return };

        let body_json: serde_json::Value = serde_json::from_str(&entry.body)
            .unwrap_or_else(|_| json!({"description": entry.body.as_str()}));

        let task_name = entry.title.clone();
        let task_description = body_json["description"].as_str().unwrap_or("").to_string();
        let diff_preview = body_json["diff_preview"]
            .as_str()
            .unwrap_or("(no diff available)")
            .to_string();
        let branch = body_json["branch"].as_str().unwrap_or("unknown").to_string();
        let cost = body_json["session_cost_usd"].as_f64();
        let tests = body_json["test_summary"].as_str().map(|s| s.to_string());
        let entry_id = entry.id;
        let agent_name = entry.source_agent.clone();

        if let Some(ws_ref) = self.workspace.as_ref().and_then(|w| w.upgrade()) {
            ws_ref.update(cx, |workspace, cx| {
                ApprovalGate::open(
                    task_name,
                    task_description,
                    branch,
                    diff_preview,
                    cost,
                    tests,
                    move |decision: ApprovalDecision| {
                        let resolution = match &decision {
                            ApprovalDecision::Approve => json!({"decision": "approve"}),
                            ApprovalDecision::RequestChanges { message } => {
                                json!({"decision": "request_changes", "message": message})
                            }
                            ApprovalDecision::Reject => json!({"decision": "reject"}),
                        };
                        let resolution_str = resolution.to_string();
                        let agent = agent_name.clone();
                        std::thread::spawn(move || {
                            let _ = handle.resolve_inbox_entry(entry_id, &resolution_str);
                            if let Some(name) = agent {
                                let state = match &decision {
                                    ApprovalDecision::Reject => AgentState::Idle,
                                    _ => AgentState::Working,
                                };
                                let _ = handle.set_agent_state(&name, state);
                            }
                        });
                    },
                    workspace,
                    window,
                    cx,
                );
            });
        }
    }
}

impl EventEmitter<ItemEvent> for InboxItem {}

impl Focusable for InboxItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for InboxItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Inbox".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::BellDot))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

impl Render for InboxItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.hq_state.read(cx);
        let all_entries = state.inbox_entries.clone();
        let unread_count = state.unread_inbox_count;
        let _ = state;

        let filter = self.filter;

        let filters = [
            InboxFilter::All,
            InboxFilter::Unread,
            InboxFilter::Approval,
            InboxFilter::Blocked,
            InboxFilter::Suggestion,
        ];

        let filter_row = h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                Label::new("INBOX")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .when(unread_count > 0, |this| {
                this.child(
                    Label::new(format!("{unread_count}"))
                        .size(LabelSize::XSmall)
                        .color(Color::Error),
                )
            })
            .flex_1()
            .children(filters.into_iter().map(|f| {
                let is_selected = filter == f;
                Button::new(f.label(), f.label())
                    .style(if is_selected {
                        ButtonStyle::Tinted(TintColor::Accent)
                    } else {
                        ButtonStyle::Subtle
                    })
                    .label_size(LabelSize::XSmall)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = f;
                        cx.notify();
                    }))
            }));

        let entries: Vec<_> = all_entries
            .into_iter()
            .filter(|e| filter.matches(e))
            .collect();

        let is_empty = entries.is_empty();

        let cards: Vec<_> = entries
            .into_iter()
            .enumerate()
            .map(|(ix, entry)| {
                let severity_color = match entry.severity {
                    InboxSeverity::Critical => Color::Error,
                    InboxSeverity::Warning => Color::Warning,
                    InboxSeverity::Info => Color::Muted,
                };
                let type_label = match entry.entry_type {
                    InboxEntryType::Approval => "approval",
                    InboxEntryType::Blocked => "blocked",
                    InboxEntryType::Suggestion => "suggestion",
                    InboxEntryType::Risk => "risk",
                    InboxEntryType::CostSpike => "cost",
                    InboxEntryType::Completed => "done",
                };
                let title = entry.title.clone();
                let body = entry.body.clone();
                let is_read = entry.read;
                let id = entry.id;
                let ref_type = entry.ref_type.clone();
                let ref_id = entry.ref_id;
                let ws = self.workspace.clone();
                let is_approval = entry.entry_type == InboxEntryType::Approval;
                let is_resolved = entry.resolved;
                let resolution_label = if is_approval && is_resolved {
                    entry
                        .resolution
                        .as_deref()
                        .and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok())
                        .and_then(|v| v["decision"].as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| "resolved".to_string())
                } else {
                    String::new()
                };

                v_flex()
                    .id(("inbox-entry", ix))
                    .w_full()
                    .p_2()
                    .gap_0p5()
                    .rounded_sm()
                    .bg(if is_read {
                        cx.theme().colors().element_background
                    } else {
                        cx.theme().colors().element_hover
                    })
                    .border_l_2()
                    .border_color(severity_color.color(cx))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Label::new(type_label)
                                    .size(LabelSize::XSmall)
                                    .color(severity_color),
                            )
                            .flex_1()
                            .child(Label::new(title).size(LabelSize::Small))
                            .when(!is_read, |this| {
                                this.child(
                                    div()
                                        .w(px(6.))
                                        .h(px(6.))
                                        .rounded_full()
                                        .flex_none()
                                        .bg(Color::Accent.color(cx)),
                                )
                            }),
                    )
                    .when(!body.is_empty(), |this| {
                        let truncated = if body.len() > 120 {
                            format!("{}…", &body[..120])
                        } else {
                            body.clone()
                        };
                        this.child(
                            Label::new(truncated)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .when_some(ref_type.as_deref(), |this, rtype| {
                                // Clicking the ref link opens the relevant view
                                let ws2 = ws.clone();
                                let rtype_str = rtype.to_string();
                                let ref_id_str = ref_id.map(|u| u.to_string());
                                let label = format!("→ {rtype}");
                                this.child(
                                    Button::new(("entry-ref", ix), label)
                                        .style(ButtonStyle::Subtle)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener(move |_, _, window, cx| {
                                            if rtype_str == "thread" {
                                                if let Some(ref name) = ref_id_str {
                                                    if let Some(ws_ref) =
                                                        ws2.as_ref().and_then(|w| w.upgrade())
                                                    {
                                                        ws_ref.update(cx, |workspace, cx| {
                                                            open_thread_view(
                                                                workspace,
                                                                name.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    }
                                                }
                                            }
                                        })),
                                )
                            })
                            .flex_1()
                            .when(!is_read, |this| {
                                this.child(
                                    Button::new(("entry-read", ix), "Mark read")
                                        .style(ButtonStyle::Subtle)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.mark_read(id, cx);
                                        })),
                                )
                            })
                            .when(is_approval && !is_resolved, |this| {
                                this.child(
                                    Button::new(("entry-review", ix), "Review")
                                        .style(ButtonStyle::Filled)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener({
                                            let entry_for_review = entry.clone();
                                            move |this, _, window, cx| {
                                                this.mark_read(entry_for_review.id, cx);
                                                this.open_approval_gate(&entry_for_review, window, cx);
                                            }
                                        })),
                                )
                            })
                            .when(is_approval && is_resolved, |this| {
                                let color = match resolution_label.as_str() {
                                    "approve" => Color::Success,
                                    "reject" => Color::Error,
                                    _ => Color::Warning,
                                };
                                this.child(
                                    Label::new(resolution_label.clone())
                                        .size(LabelSize::XSmall)
                                        .color(color),
                                )
                            })
                            .child(
                                Button::new(("entry-dismiss", ix), "Dismiss")
                                    .style(ButtonStyle::Subtle)
                                    .label_size(LabelSize::XSmall)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.dismiss_entry(id, cx);
                                    })),
                            ),
                    )
            })
            .collect();

        v_flex()
            .key_context("Inbox")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .child(filter_row)
            .child(
                v_flex()
                    .id("inbox-entries")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .when(is_empty, |this| {
                        this.child(
                            v_flex()
                                .pt_8()
                                .items_center()
                                .gap_2()
                                .child(
                                    Label::new("Inbox zero")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(
                                    Label::new("No items requiring attention")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        )
                    })
                    .children(cards),
            )
    }
}

/// Open or activate the Inbox in the active workspace.
pub fn open_inbox(
    workspace: &mut workspace::Workspace,
    hq_state: Entity<HqState>,
    window: &mut Window,
    cx: &mut Context<workspace::Workspace>,
) {
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<InboxItem>());

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let weak_workspace = cx.weak_entity();
        let item = cx.new(|cx: &mut Context<InboxItem>| {
            InboxItem::new(hq_state, Some(weak_workspace), window, cx)
        });
        workspace.add_item_to_center(Box::new(item), window, cx);
    }
}
