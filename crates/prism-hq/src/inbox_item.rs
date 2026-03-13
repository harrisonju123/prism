use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, Styled, Subscription, Task, WeakEntity,
    Window, actions,
};
use prism_context::model::{InboxEntry, InboxEntryType, InboxSeverity};
use ui::{
    Button, ButtonStyle, Color, Icon, IconName, Label, LabelSize, TintColor, h_flex, prelude::*,
    v_flex,
};
use workspace::Workspace;
use workspace::item::{Item, ItemEvent};

use crate::approval_gate::{ApprovalDecision, ApprovalGate};
use crate::context_service::{ContextHandle, ContextService, get_context_handle};
use crate::dispatch::slugify;
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
    CostSpike,
    Completed,
    Suggestion,
}

impl InboxFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Unread => "Unread",
            Self::Approval => "Approval",
            Self::Blocked => "Blocked",
            Self::CostSpike => "Cost",
            Self::Completed => "Done",
            Self::Suggestion => "Suggestion",
        }
    }

    fn matches(self, entry: &InboxEntry) -> bool {
        match self {
            Self::All => true,
            Self::Unread => !entry.read,
            Self::Approval => entry.entry_type == InboxEntryType::Approval,
            Self::Blocked => entry.entry_type == InboxEntryType::Blocked,
            Self::CostSpike => entry.entry_type == InboxEntryType::CostSpike,
            Self::Completed => entry.entry_type == InboxEntryType::Completed,
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
    /// Active unblock composer: (entry_id, message_buffer). None when closed.
    unblock_state: Option<(uuid::Uuid, String)>,
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
            unblock_state: None,
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

        let mut packet =
            crate::review_packet::ReviewPacket::from_inbox_body(&entry.title, &entry.body);

        // Infer branch from source_agent if not set by producer.
        if packet.branch.is_empty() {
            if let Some(ref agent) = entry.source_agent {
                packet.branch = agent.clone();
            }
        }

        // Clone once; reused for both enrich_from_context and enrich_diff.
        let branch = packet.branch.clone();

        // Enrich description from the thread (uses branch name as thread name).
        packet.enrich_from_context(&handle, &branch);

        // Enrich test summary from RunningAgents output for this agent.
        let agent_output: Vec<String> = cx
            .try_global::<crate::running_agents::RunningAgentsGlobal>()
            .map(|g| g.0.read(cx).output_lines(&branch))
            .unwrap_or_default();
        packet.enrich_test_summary(&agent_output);

        // Run git diff synchronously (typically fast < 1s for local repos).
        packet.enrich_diff(&branch);

        let entry_id = entry.id;
        let agent_name = entry.source_agent.clone();

        if let Some(ws_ref) = self.workspace.as_ref().and_then(|w| w.upgrade()) {
            ws_ref.update(cx, |workspace, cx| {
                ApprovalGate::open(
                    packet.task_name,
                    packet.description,
                    packet.branch,
                    packet.diff_preview,
                    packet.session_cost_usd,
                    packet.test_summary,
                    move |decision: ApprovalDecision| {
                        let resolution = match &decision {
                            ApprovalDecision::Approve => json!({"decision": "approve"}),
                            ApprovalDecision::RequestChanges { message } => {
                                json!({"decision": "request_changes", "message": message})
                            }
                            ApprovalDecision::Reject => json!({"decision": "reject"}),
                        };
                        let resolution_str = resolution.to_string();
                        std::thread::spawn(move || {
                            let _ = handle.resolve_inbox_entry(entry_id, &resolution_str);
                            if let Some(name) = agent_name {
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

    fn unblock_agent(&mut self, entry: &InboxEntry, cx: &mut Context<Self>) {
        use prism_context::model::AgentState;
        use serde_json::json;

        let is_open_for_entry = self
            .unblock_state
            .as_ref()
            .map_or(false, |(id, _)| *id == entry.id);
        let has_message = self
            .unblock_state
            .as_ref()
            .map_or(false, |(id, msg)| *id == entry.id && !msg.is_empty());

        if !has_message {
            // Toggle composer open/closed.
            self.unblock_state = if is_open_for_entry {
                None
            } else {
                Some((entry.id, String::new()))
            };
            cx.notify();
            return;
        }

        let id = entry.id;
        let (_, message) = self.unblock_state.take().unwrap();
        let agent_name = entry.source_agent.clone();

        self.pending_op = Some(cx.spawn(async move |this, cx| {
            if let Some(handle) = get_context_handle(&this, cx) {
                let resolution = json!({"action": "unblocked", "message": message}).to_string();
                let _ = cx
                    .background_spawn(async move {
                        let _ = handle.resolve_inbox_entry(id, &resolution);
                        if let Some(name) = agent_name {
                            let _ = handle.send_message("supervisor", &name, &message);
                            let _ = handle.set_agent_state(&name, AgentState::Working);
                        }
                    })
                    .await;
            }

            this.update(cx, |_, cx| cx.notify()).ok();
        }));
    }

    fn kill_agent(&mut self, entry: &InboxEntry, cx: &mut Context<Self>) {
        use prism_context::model::AgentState;
        use serde_json::json;

        let id = entry.id;
        let agent_name = entry.source_agent.clone();

        self.pending_op = Some(cx.spawn(async move |this, cx| {
            if let Some(handle) = get_context_handle(&this, cx) {
                let resolution = json!({"action": "killed"}).to_string();
                let _ = cx
                    .background_spawn(async move {
                        let _ = handle.resolve_inbox_entry(id, &resolution);
                        if let Some(name) = agent_name {
                            let _ = handle.set_agent_state(&name, AgentState::Dead);
                        }
                    })
                    .await;
            }

            this.update(cx, |_, cx| cx.notify()).ok();
        }));
    }

    fn merge_completed(&mut self, entry: &InboxEntry, cx: &mut Context<Self>) {
        use serde_json::json;

        let id = entry.id;
        let branch = {
            let body_json: serde_json::Value =
                serde_json::from_str(&entry.body).unwrap_or(json!({}));
            body_json["branch"].as_str().unwrap_or("").to_string()
        };

        self.pending_op = Some(cx.spawn(async move |this, cx| {
            if let Some(handle) = get_context_handle(&this, cx) {
                let _ = cx
                    .background_spawn(async move {
                        let output = std::process::Command::new("git")
                            .args(["merge", "--no-ff", &branch])
                            .output();
                        let resolution = match output {
                            Ok(out) if out.status.success() => {
                                json!({"action": "merged"}).to_string()
                            }
                            Ok(out) => {
                                let err = String::from_utf8_lossy(&out.stderr).to_string();
                                json!({"action": "merge_failed", "error": err}).to_string()
                            }
                            Err(e) => {
                                json!({"action": "merge_failed", "error": e.to_string()})
                                    .to_string()
                            }
                        };
                        let _ = handle.resolve_inbox_entry(id, &resolution);
                    })
                    .await;
            }

            this.update(cx, |_, cx| cx.notify()).ok();
        }));
    }

    fn create_thread_from_suggestion(&mut self, entry: &InboxEntry, cx: &mut Context<Self>) {
        use serde_json::json;

        let id = entry.id;
        let body = entry.body.clone();
        let thread_name = slugify(&entry.title);

        self.pending_op = Some(cx.spawn(async move |this, cx| {
            if let Some(handle) = get_context_handle(&this, cx) {
                let _ = cx
                    .background_spawn(async move {
                        let _ = handle.create_thread(
                            &thread_name,
                            &body,
                            vec!["from-suggestion".to_string()],
                        );
                        let resolution =
                            json!({"action": "thread_created", "thread": thread_name}).to_string();
                        let _ = handle.resolve_inbox_entry(id, &resolution);
                    })
                    .await;
            }

            this.update(cx, |_, cx| cx.notify()).ok();
        }));
    }

    fn acknowledge_risk(&mut self, entry: &InboxEntry, cx: &mut Context<Self>) {
        use serde_json::json;

        let id = entry.id;

        self.pending_op = Some(cx.spawn(async move |this, cx| {
            if let Some(handle) = get_context_handle(&this, cx) {
                let resolution = json!({"action": "acknowledged"}).to_string();
                let _ = cx
                    .background_spawn(async move { handle.resolve_inbox_entry(id, &resolution) })
                    .await;
            }

            this.update(cx, |_, cx| cx.notify()).ok();
        }));
    }

    fn escalate_risk(
        &mut self,
        entry: &InboxEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use serde_json::json;

        let id = entry.id;
        let thread_name = entry.title.clone();

        if let Some(ws_ref) = self.workspace.as_ref().and_then(|w| w.upgrade()) {
            ws_ref.update(cx, |workspace, cx| {
                open_thread_view(workspace, thread_name, window, cx);
            });
        }

        self.pending_op = Some(cx.spawn(async move |this, cx| {
            if let Some(handle) = get_context_handle(&this, cx) {
                let resolution = json!({"action": "escalated"}).to_string();
                let _ = cx
                    .background_spawn(async move { handle.resolve_inbox_entry(id, &resolution) })
                    .await;
            }

            this.update(cx, |_, cx| cx.notify()).ok();
        }));
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
            InboxFilter::CostSpike,
            InboxFilter::Completed,
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

        // Capture unblock state for use in card closures.
        let unblock_target = self.unblock_state.as_ref().map(|(id, _)| *id);
        let unblock_message = self
            .unblock_state
            .as_ref()
            .map(|(_, msg)| msg.clone())
            .unwrap_or_default();

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
                let is_blocked = entry.entry_type == InboxEntryType::Blocked;
                let is_cost_spike = entry.entry_type == InboxEntryType::CostSpike;
                let is_completed = entry.entry_type == InboxEntryType::Completed;
                let is_suggestion = entry.entry_type == InboxEntryType::Suggestion;
                let is_risk = entry.entry_type == InboxEntryType::Risk;
                let is_resolved = entry.resolved;
                let show_unblock_form =
                    is_blocked && !is_resolved && unblock_target == Some(id);

                // Generalized resolution label: prefer "action", fall back to "decision".
                let resolution_label = if is_resolved {
                    entry
                        .resolution
                        .as_deref()
                        .and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok())
                        .and_then(|v| {
                            v["action"]
                                .as_str()
                                .or_else(|| v["decision"].as_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_else(|| "resolved".to_string())
                } else {
                    String::new()
                };

                // Inline message from resolution JSON (shown on Blocked and Approval entries).
                let resolution_message = if is_resolved {
                    entry
                        .resolution
                        .as_deref()
                        .and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok())
                        .and_then(|v| v["message"].as_str().map(|s| s.to_string()))
                } else {
                    None
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
                    // Unblock inline composer (visible when composer is open for this entry).
                    .when(show_unblock_form, |this| {
                        let display_text = if unblock_message.is_empty() {
                            "Type a message… (Esc to cancel)".to_string()
                        } else {
                            format!("{}|", unblock_message)
                        };
                        let text_color = if unblock_message.is_empty() {
                            Color::Muted
                        } else {
                            Color::Default
                        };
                        this.child(
                            div()
                                .mt_0p5()
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .bg(cx.theme().colors().editor_background)
                                .border_1()
                                .border_color(cx.theme().colors().border_focused)
                                .child(
                                    Label::new(display_text)
                                        .size(LabelSize::XSmall)
                                        .color(text_color),
                                ),
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
                            // Approval actions
                            .when(is_approval && !is_resolved, |this| {
                                this.child(
                                    Button::new(("entry-review", ix), "Review")
                                        .style(ButtonStyle::Filled)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener({
                                            let entry_for_review = entry.clone();
                                            move |this, _, window, cx| {
                                                this.mark_read(entry_for_review.id, cx);
                                                this.open_approval_gate(
                                                    &entry_for_review,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        })),
                                )
                            })
                            // Blocked actions
                            .when(is_blocked && !is_resolved, |this| {
                                let entry_clone = entry.clone();
                                let has_msg = !unblock_message.is_empty()
                                    && unblock_target == Some(id);
                                let btn_label = if has_msg {
                                    "Send & Unblock"
                                } else {
                                    "Unblock"
                                };
                                this.child(
                                    Button::new(("entry-unblock", ix), btn_label)
                                        .style(ButtonStyle::Filled)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.unblock_agent(&entry_clone, cx);
                                        })),
                                )
                            })
                            // CostSpike actions
                            .when(is_cost_spike && !is_resolved, |this| {
                                let entry_clone = entry.clone();
                                this.child(
                                    Button::new(("entry-kill", ix), "Kill Agent")
                                        .style(ButtonStyle::Filled)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.kill_agent(&entry_clone, cx);
                                        })),
                                )
                            })
                            // Completed actions
                            .when(is_completed && !is_resolved, |this| {
                                let entry_review = entry.clone();
                                let entry_merge = entry.clone();
                                this.child(
                                    Button::new(("entry-review-completed", ix), "Review")
                                        .style(ButtonStyle::Filled)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener({
                                            move |this, _, window, cx| {
                                                this.mark_read(entry_review.id, cx);
                                                this.open_approval_gate(
                                                    &entry_review,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        })),
                                )
                                .child(
                                    Button::new(("entry-merge", ix), "Merge")
                                        .style(ButtonStyle::Subtle)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.merge_completed(&entry_merge, cx);
                                        })),
                                )
                            })
                            // Suggestion actions
                            .when(is_suggestion && !is_resolved, |this| {
                                let entry_clone = entry.clone();
                                this.child(
                                    Button::new(("entry-thread", ix), "Create Thread")
                                        .style(ButtonStyle::Filled)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.create_thread_from_suggestion(&entry_clone, cx);
                                        })),
                                )
                            })
                            // Risk actions
                            .when(is_risk && !is_resolved, |this| {
                                let entry_ack = entry.clone();
                                let entry_esc = entry.clone();
                                this.child(
                                    Button::new(("entry-ack", ix), "Acknowledge")
                                        .style(ButtonStyle::Subtle)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.acknowledge_risk(&entry_ack, cx);
                                        })),
                                )
                                .child(
                                    Button::new(("entry-escalate", ix), "Escalate")
                                        .style(ButtonStyle::Filled)
                                        .label_size(LabelSize::XSmall)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.escalate_risk(&entry_esc, window, cx);
                                        })),
                                )
                            })
                            // Resolved label for all types
                            .when(is_resolved, |this| {
                                let color = match resolution_label.as_str() {
                                    "approve" | "merged" | "acknowledged" | "thread_created" => {
                                        Color::Success
                                    }
                                    "reject" | "killed" => Color::Error,
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
                    .when_some(resolution_message, |this, msg| {
                        this.child(
                            div()
                                .mt_0p5()
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .bg(cx.theme().colors().editor_background)
                                .child(
                                    Label::new(format!("↩ {msg}"))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        )
                    })
            })
            .collect();

        v_flex()
            .key_context("Inbox")
            .track_focus(&self.focus_handle)
            // Keyboard handler for unblock composer (active when any composer is open).
            .when(self.unblock_state.is_some(), |this| {
                this.on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    let ks = &event.keystroke;
                    if ks.key == "escape" {
                        this.unblock_state = None;
                        cx.notify();
                    } else if ks.key == "backspace" {
                        if let Some((_, msg)) = this.unblock_state.as_mut() {
                            if !msg.is_empty() {
                                msg.pop();
                                cx.notify();
                            }
                        }
                    } else if !ks.modifiers.platform && !ks.modifiers.control {
                        if let Some(ch) = &ks.key_char {
                            if let Some((_, msg)) = this.unblock_state.as_mut() {
                                msg.push_str(ch);
                                cx.notify();
                            }
                        }
                    }
                }))
            })
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
