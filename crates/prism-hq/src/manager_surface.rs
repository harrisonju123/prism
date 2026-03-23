use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, WeakEntity, Window, actions, px,
};
use prism_context::model::{AgentState, InboxEntryType, InboxSeverity, Plan};
use uuid::Uuid;
use ui::{
    Button, ButtonStyle, Color, Icon, IconName, Label, LabelSize, h_flex, prelude::*, v_flex,
};
use workspace::Workspace;
use workspace::item::{Item, ItemEvent};

use crate::activity_bus;
use crate::agent_view::open_agent_view;
use crate::approval_gate::{ApprovalDecision, ApprovalGate};
use crate::context_service::ContextService;
use crate::hq_state::HqState;
use crate::review_packet::ReviewPacket;
use crate::running_agents::RunningAgents;

actions!(prism_hq, [OpenManagerSurface]);

/// Computed display snapshot for a single agent card. Rebuilt on each HqState notification.
struct AgentCard {
    name: String,
    state: AgentState,
    current_thread: Option<String>,
    /// True if this agent was spawned from this IDE session and is still running.
    is_local_running: bool,
    /// True if this agent was spawned from this IDE session and has since exited.
    is_local_completed: bool,
    /// Last 5 lines of output from the RunningAgents ring buffer.
    output_preview: Vec<String>,
}

/// An actionable inbox entry surfaced in the Artifacts section.
struct ArtifactEntry {
    id: Uuid,
    title: String,
    severity: InboxSeverity,
    entry_type: InboxEntryType,
    source_agent: Option<String>,
    /// Raw body for ReviewPacket parsing.
    body: String,
}

pub struct ManagerSurface {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    agent_cards: Vec<AgentCard>,
    artifacts: Vec<ArtifactEntry>,
    active_plan: Option<Plan>,
    cumulative_cost_usd: f64,
    /// Live IDE agent status from AgentActivityBus (this Claude Code session).
    ide_is_generating: bool,
    ide_live_tool: Option<String>,
    ide_live_file: Option<String>,
    _hq_sub: Option<gpui::Subscription>,
    _activity_sub: Option<gpui::Subscription>,
    _running_agents_sub: Option<gpui::Subscription>,
}

impl ManagerSurface {
    pub fn new(workspace: WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify()).detach();

        // Subscribe to HqState — rebuilds all card/artifact data on every 10s poll.
        let hq_sub = HqState::global(cx).map(|hq_entity| {
            cx.observe(&hq_entity, |this, hq_entity, cx| {
                let hq = hq_entity.read(cx);
                let ra_opt = RunningAgents::global(cx);

                this.agent_cards = hq.agents.iter().map(|a| {
                    let (is_running, is_completed, preview) = ra_opt.as_ref()
                        .map(|ra| {
                            let ra = ra.read(cx);
                            let lines = ra.output_lines(&a.name);
                            let preview: Vec<String> = lines.iter().rev().take(5).rev().cloned().collect();
                            (ra.is_running(&a.name), ra.is_completed(&a.name), preview)
                        })
                        .unwrap_or_default();
                    AgentCard {
                        name: a.name.clone(),
                        state: a.state.clone(),
                        current_thread: a.current_thread.clone(),
                        is_local_running: is_running,
                        is_local_completed: is_completed,
                        output_preview: preview,
                    }
                }).collect();

                this.artifacts = hq.inbox_entries.iter()
                    .filter(|e| !e.read && !e.dismissed && !e.resolved)
                    .map(|e| ArtifactEntry {
                        id: e.id,
                        title: e.title.clone(),
                        severity: e.severity.clone(),
                        entry_type: e.entry_type.clone(),
                        source_agent: e.source_agent.clone(),
                        body: e.body.clone(),
                    })
                    .collect();

                this.active_plan = hq.active_plan.clone();
                this.cumulative_cost_usd = hq.cumulative_cost_usd;
                cx.notify();
            })
        });

        // Subscribe to RunningAgents — updates output preview and running/completed flags.
        let ra_sub = RunningAgents::global(cx).map(|ra_entity| {
            cx.observe(&ra_entity, |this, ra_entity, cx| {
                let ra = ra_entity.read(cx);
                let mut changed = false;
                for card in &mut this.agent_cards {
                    let new_running = ra.is_running(&card.name);
                    let new_completed = ra.is_completed(&card.name);
                    let lines = ra.output_lines(&card.name);
                    let preview: Vec<String> = lines.iter().rev().take(5).rev().cloned().collect();
                    if card.is_local_running != new_running
                        || card.is_local_completed != new_completed
                        || card.output_preview != preview
                    {
                        card.is_local_running = new_running;
                        card.is_local_completed = new_completed;
                        card.output_preview = preview;
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            })
        });

        // Subscribe to ActivityBus — tracks this IDE session's live tool/file activity.
        let activity_sub = activity_bus::global_inner(cx).map(|bus_entity| {
            cx.observe(&bus_entity, |this, bus_entity, cx| {
                let bus = bus_entity.read(cx);
                let new_gen = bus.is_generating;
                let new_tool = bus.current_tool.clone();
                let new_file = bus.current_file.clone();
                if this.ide_is_generating != new_gen
                    || this.ide_live_tool != new_tool
                    || this.ide_live_file != new_file
                {
                    this.ide_is_generating = new_gen;
                    this.ide_live_tool = new_tool;
                    this.ide_live_file = new_file;
                    cx.notify();
                }
            })
        });

        Self {
            focus_handle,
            workspace,
            agent_cards: Vec::new(),
            artifacts: Vec::new(),
            active_plan: None,
            cumulative_cost_usd: 0.0,
            ide_is_generating: false,
            ide_live_tool: None,
            ide_live_file: None,
            _hq_sub: hq_sub,
            _activity_sub: activity_sub,
            _running_agents_sub: ra_sub,
        }
    }

    /// Enriches `packet` from context + agent output + git diff, then opens the ApprovalGate.
    /// `inbox_id` is `Some` when the review originates from an inbox entry (triggers resolution on decision).
    fn enrich_and_open_gate(
        &mut self,
        mut packet: ReviewPacket,
        agent_name: String,
        inbox_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = cx.try_global::<ContextService>().and_then(|svc| svc.handle());
        if let Some(ref h) = handle {
            packet.enrich_from_context(h, &agent_name);
        }
        let output = RunningAgents::global(cx)
            .map(|ra| ra.read(cx).output_lines(&agent_name))
            .unwrap_or_default();
        packet.enrich_test_summary(&output);
        // Use the packet's own branch if set (e.g. from an inbox body); fall back to agent name.
        let branch = if packet.branch.is_empty() { agent_name.clone() } else { packet.branch.clone() };
        if !branch.is_empty() {
            packet.enrich_diff(&branch);
        }
        if let Some(ws) = self.workspace.upgrade() {
            ws.update(cx, |workspace, cx| {
                ApprovalGate::open(
                    packet.task_name,
                    packet.description,
                    packet.branch,
                    packet.diff_preview,
                    packet.session_cost_usd,
                    packet.test_summary,
                    move |decision: ApprovalDecision| {
                        if let Some(h) = handle {
                            let name = agent_name.clone();
                            std::thread::spawn(move || {
                                let _ = crate::decision_executor::execute_decision(
                                    decision, h, name.clone(), inbox_id, Some(name), None,
                                );
                            });
                        }
                    },
                    workspace,
                    window,
                    cx,
                );
            });
        }
    }

    fn review_agent_by_name(&mut self, agent_name: String, window: &mut Window, cx: &mut Context<Self>) {
        let packet = ReviewPacket {
            task_name: agent_name.clone(),
            branch: agent_name.clone(),
            ..Default::default()
        };
        self.enrich_and_open_gate(packet, agent_name, None, window, cx);
    }

    fn review_artifact(&mut self, artifact_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(artifact) = self.artifacts.iter().find(|a| a.id == artifact_id) else {
            return;
        };
        let packet = ReviewPacket::from_inbox_body(&artifact.title, &artifact.body);
        let agent_name = artifact.source_agent.clone().unwrap_or_default();
        self.enrich_and_open_gate(packet, agent_name, Some(artifact_id), window, cx);
    }
}

impl EventEmitter<ItemEvent> for ManagerSurface {}

impl Focusable for ManagerSurface {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for ManagerSurface {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &gpui::App) -> SharedString {
        "Manager".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &gpui::App) -> Option<ui::Icon> {
        Some(Icon::new(IconName::Person))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

impl Render for ManagerSurface {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plan_label: Option<SharedString> = self.active_plan.as_ref().map(|p| {
            SharedString::from(p.intent.chars().take(40).collect::<String>())
        });
        let cost = self.cumulative_cost_usd;
        let ide_is_generating = self.ide_is_generating;
        let ide_live_tool = self.ide_live_tool.clone();
        let ide_live_file = self.ide_live_file.clone();
        let has_artifacts = !self.artifacts.is_empty();

        let ws_weak = self.workspace.clone();

        let cards: Vec<_> = self
            .agent_cards
            .iter()
            .enumerate()
            .map(|(idx, card)| {
                let name = card.name.clone();
                let open_name = name.clone();
                let review_name = name.clone();
                let state_color = match card.state {
                    AgentState::Working => Color::Accent,
                    AgentState::Idle => Color::Success,
                    AgentState::Blocked => Color::Warning,
                    AgentState::Dead => Color::Muted,
                    AgentState::AwaitingReview => Color::Warning,
                };
                let state_label = SharedString::from(card.state.to_string());
                let current_thread = card.current_thread.clone();
                let is_completed = card.is_local_completed;
                let output_preview = card.output_preview.clone();

                v_flex()
                    .id(SharedString::from(format!("agent_card_{idx}")))
                    .p_2()
                    .gap_1()
                    .rounded_md()
                    .bg(cx.theme().colors().element_background)
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .cursor_pointer()
                    .on_click({
                        // Use a plain closure (not cx.listener) so ManagerSurface is not
                        // mutably borrowed when the handler runs. If we used cx.listener here,
                        // workspace.add_item_to_center → Pane::add_item_inner → buffer_kind
                        // would try to read() ManagerSurface while it's already mutably held,
                        // causing a double_lease_panic.
                        let ws = ws_weak.clone();
                        let n = open_name.clone();
                        move |_, window, cx| {
                            if let Some(ws) = ws.upgrade() {
                                ws.update(cx, |workspace, cx| {
                                    open_agent_view(workspace, n.clone(), window, cx);
                                });
                            }
                        }
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Label::new(SharedString::from(name)).size(LabelSize::Small))
                            .child(Label::new(state_label).size(LabelSize::XSmall).color(state_color)),
                    )
                    .when_some(current_thread, |this, thread| {
                        this.child(
                            Label::new(format!("↳ {thread}"))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .when(!output_preview.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .gap_0p5()
                                .p_1()
                                .rounded_sm()
                                .bg(cx.theme().colors().editor_background)
                                .children(output_preview.into_iter().map(|line| {
                                    Label::new(line).size(LabelSize::XSmall).color(Color::Default)
                                })),
                        )
                    })
                    .when(is_completed, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("review_{review_name}")), "Review")
                                .style(ButtonStyle::Filled)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.review_agent_by_name(review_name.clone(), window, cx);
                                })),
                        )
                    })
            })
            .collect();

        let artifact_rows: Vec<_> = self
            .artifacts
            .iter()
            .map(|artifact| {
                let severity_color = match &artifact.severity {
                    InboxSeverity::Critical => Color::Error,
                    InboxSeverity::Warning => Color::Warning,
                    InboxSeverity::Info => Color::Muted,
                };
                let severity_dot = match &artifact.severity {
                    InboxSeverity::Critical => "⚠",
                    InboxSeverity::Warning => "◆",
                    InboxSeverity::Info => "·",
                };
                let title = SharedString::from(artifact.title.clone());
                let source_label: Option<SharedString> = artifact
                    .source_agent
                    .as_ref()
                    .map(|a| SharedString::from(format!("from {a}")));
                let entry_type = artifact.entry_type.clone();
                let artifact_id = artifact.id;
                let source_for_unblock = artifact.source_agent.clone();

                h_flex()
                    .id(SharedString::from(format!("artifact_{artifact_id}")))
                    .p_1p5()
                    .gap_1()
                    .rounded_sm()
                    .bg(cx.theme().colors().element_background)
                    .child(Label::new(severity_dot).size(LabelSize::Small).color(severity_color))
                    .child(
                        v_flex()
                            .flex_1()
                            .child(Label::new(title).size(LabelSize::Small))
                            .when_some(source_label, |this, src| {
                                this.child(Label::new(src).size(LabelSize::XSmall).color(Color::Muted))
                            }),
                    )
                    .when(
                        matches!(entry_type, InboxEntryType::Completed | InboxEntryType::Approval),
                        |this| {
                            this.child(
                                Button::new(
                                    SharedString::from(format!("artifact_review_{artifact_id}")),
                                    "Review",
                                )
                                .style(ButtonStyle::Filled)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.review_artifact(artifact_id, window, cx);
                                })),
                            )
                        },
                    )
                    .when(matches!(entry_type, InboxEntryType::Blocked), |this| {
                        this.child(
                            Button::new(
                                SharedString::from(format!("artifact_unblock_{artifact_id}")),
                                "Unblock",
                            )
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click({
                                let ws = ws_weak.clone();
                                let src = source_for_unblock.clone();
                                move |_, window, cx| {
                                    if let Some(src) = src.clone() {
                                        if let Some(ws) = ws.upgrade() {
                                            ws.update(cx, |workspace, cx| {
                                                open_agent_view(workspace, src, window, cx);
                                            });
                                        }
                                    }
                                }
                            }),
                        )
                    })
            })
            .collect();

        v_flex()
            .key_context("ManagerSurface")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .h(px(40.))
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .gap_2()
                    .child(Label::new("Manager Surface").size(LabelSize::Small).color(Color::Muted))
                    .when_some(plan_label, |this, label| {
                        this.child(Label::new(label).size(LabelSize::XSmall).color(Color::Accent))
                    })
                    .when(cost > 0.0, |this| {
                        this.child(
                            Label::new(format!("${cost:.2}")).size(LabelSize::XSmall).color(Color::Muted),
                        )
                    })
                    .when(ide_is_generating, |this| {
                        let activity = if let Some(tool) = &ide_live_tool {
                            if let Some(file) = &ide_live_file {
                                let short = std::path::Path::new(file.as_str())
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(file.as_str())
                                    .to_string();
                                format!("{tool}: {short}")
                            } else {
                                format!("{tool}…")
                            }
                        } else {
                            "generating…".to_string()
                        };
                        this.child(Label::new(activity).size(LabelSize::XSmall).color(Color::Accent))
                    })
                    .flex_1()
                    .justify_end()
                    .child(
                        Button::new("dispatch_task", "Dispatch Task")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(ws) = this.workspace.upgrade() {
                                    let ws_weak = ws.downgrade();
                                    ws.update(cx, |workspace, cx| {
                                        workspace.toggle_modal(window, cx, move |window, cx| {
                                            crate::TaskDispatchModal::new(ws_weak.clone(), window, cx)
                                        });
                                    });
                                }
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("manager_body")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_2()
                    .gap_3()
                    .child(Label::new("AGENTS").size(LabelSize::XSmall).color(Color::Muted))
                    .when(cards.is_empty(), |this| {
                        this.child(
                            Label::new("No active agents").size(LabelSize::Small).color(Color::Muted),
                        )
                    })
                    .children(cards)
                    .when(has_artifacts, |this| {
                        this.child(
                            Label::new("NEEDS ATTENTION").size(LabelSize::XSmall).color(Color::Warning),
                        )
                        .children(artifact_rows)
                    }),
            )
    }
}

/// Open or activate the Manager Surface tab in the workspace center pane. Singleton.
pub fn open_manager_surface(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<ManagerSurface>());
    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let ws_weak = cx.weak_entity();
        let item = cx.new(|cx| ManagerSurface::new(ws_weak, window, cx));
        workspace.add_item_to_center(Box::new(item), window, cx);
    }
}
