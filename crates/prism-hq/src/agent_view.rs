use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    SharedString, Styled, Task, WeakEntity, Window, actions,
};
use prism_context::model::{AgentSession, AgentState, AgentStatus, Risk, RiskSeverity};
use ui::{
    Button, ButtonStyle, Color, Icon, IconName, Label, LabelSize, TintColor, h_flex, prelude::*,
    v_flex,
};
use workspace::Workspace;
use workspace::item::{Item, ItemEvent};

use crate::approval_gate::{ApprovalDecision, ApprovalGate};
use crate::context_service::ContextService;
use crate::review_packet::ReviewPacket;

use crate::running_agents::RunningAgents;

actions!(prism_hq, [OpenAgentView]);

pub struct AgentViewItem {
    focus_handle: FocusHandle,
    agent_name: String,
    agent_status: Option<AgentStatus>,
    sessions: Vec<AgentSession>,
    risks: Vec<Risk>,
    is_loading: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
    workspace: Option<WeakEntity<Workspace>>,
}

impl AgentViewItem {
    pub fn new(
        agent_name: String,
        workspace: Option<WeakEntity<Workspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let auto_refresh = cx.spawn(async move |this: WeakEntity<AgentViewItem>, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                // Stop polling once the agent has reached a terminal state.
                let is_dead = this
                    .read_with(cx, |item, _| {
                        matches!(
                            item.agent_status.as_ref().map(|s| &s.state),
                            Some(AgentState::Dead)
                        )
                    })
                    .unwrap_or(true);
                if is_dead {
                    break;
                }
                this.update(cx, |item, cx| item.refresh(cx)).ok();
            }
        });

        let mut item = AgentViewItem {
            focus_handle,
            agent_name,
            agent_status: None,
            sessions: Vec::new(),
            risks: Vec::new(),
            is_loading: false,
            error: None,
            refresh_task: None,
            _auto_refresh: auto_refresh,
            workspace,
        };
        item.refresh(cx);
        item
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.is_loading = true;
        cx.notify();

        let agent_name = self.agent_name.clone();

        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<ContextService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<(Option<AgentStatus>, Vec<AgentSession>, Vec<Risk>)> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("context service not available"))?;
                    let agents = handle.list_agents()?;
                    let overview = handle.get_workspace_overview()?;
                    let agent_status = agents.into_iter().find(|a| a.name == agent_name);
                    let sessions = overview.recent_sessions;
                    let risks = handle.list_unverified_risks(None).unwrap_or_default();
                    anyhow::Ok((agent_status, sessions, risks))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok((agent_status, sessions, risks)) => {
                        this.agent_status = agent_status;
                        this.sessions = sessions;
                        this.risks = risks;
                        this.error = None;
                    }
                    Err(e) => {
                        this.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_state(&mut self, state: AgentState, cx: &mut Context<Self>) {
        let agent_name = self.agent_name.clone();
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<ContextService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<()> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("context service not available"))?;
                    handle.set_agent_state(&agent_name, state)
                })
                .await;

            this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.error = Some(e.to_string());
                }
                this.refresh(cx);
            })
            .ok();
        }));
    }

    fn kill_agent(&mut self, cx: &mut Context<Self>) {
        let agent_name = self.agent_name.clone();
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<ContextService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<()> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("context service not available"))?;
                    handle.set_agent_state(&agent_name, AgentState::Dead)
                })
                .await;

            this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.error = Some(e.to_string());
                }
                this.refresh(cx);
            })
            .ok();
        }));
    }

    fn review_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let agent_name = self.agent_name.clone();

        let handle = cx
            .try_global::<ContextService>()
            .and_then(|svc| svc.handle());

        let mut packet = ReviewPacket {
            task_name: agent_name.clone(),
            branch: agent_name.clone(),
            ..Default::default()
        };

        if let Some(ref h) = handle {
            packet.enrich_from_context(h, &agent_name);
        }

        let agent_output = RunningAgents::global(cx)
            .map(|ra| ra.read(cx).output_lines(&agent_name))
            .unwrap_or_default();
        packet.enrich_test_summary(&agent_output);
        packet.enrich_diff(&agent_name);

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
                        if let Some(h) = handle {
                            let name = agent_name.clone();
                            std::thread::spawn(move || {
                                let _ = crate::decision_executor::execute_decision(
                                    decision,
                                    h,
                                    name.clone(),
                                    None,
                                    Some(name),
                                    None,
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
}

impl EventEmitter<ItemEvent> for AgentViewItem {}

impl Focusable for AgentViewItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for AgentViewItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!("Agent: {}", self.agent_name).into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Person))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

impl Render for AgentViewItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let agent_name: SharedString = self.agent_name.clone().into();
        let is_loading = self.is_loading;
        let error = self.error.clone();
        let sessions = self.sessions.clone();
        let risks = self.risks.clone();

        let (state_label, state_color, current_thread) = if let Some(ref status) = self.agent_status
        {
            let color = match status.state {
                AgentState::Working => Color::Accent,
                AgentState::Idle => Color::Success,
                AgentState::Blocked => Color::Warning,
                AgentState::Dead => Color::Muted,
                AgentState::AwaitingReview => Color::Warning,
            };
            (
                status.state.to_string(),
                color,
                status.current_thread.clone(),
            )
        } else {
            ("unknown".to_string(), Color::Muted, None)
        };

        v_flex()
            .key_context("AgentView")
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
                    .child(Label::new(agent_name).size(LabelSize::Small))
                    .child(
                        Label::new(state_label)
                            .size(LabelSize::XSmall)
                            .color(state_color),
                    ),
            )
            .when_some(RunningAgents::global(cx), |this, running_agents| {
                let ready = running_agents.read(cx).is_completed(&self.agent_name);
                this.when(ready, |this| {
                    this.child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .h(px(32.))
                            .bg(cx.theme().colors().editor_subheader_background)
                            .border_b_1()
                            .border_color(cx.theme().colors().border)
                            .gap_1()
                            .child(
                                Label::new("Ready for Review")
                                    .size(LabelSize::Small)
                                    .color(Color::Success),
                            )
                            .child(
                                Label::new("Agent has finished its work")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(
                                Button::new("review-agent", "Review")
                                    .style(ButtonStyle::Filled)
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.review_agent(window, cx);
                                    })),
                            ),
                    )
                })
            })
            .child(
                v_flex()
                    .id("agent-content")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .when(is_loading, |this| {
                        this.child(
                            Label::new("Loading…")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .when_some(error, |this, err| {
                        this.child(Label::new(err).size(LabelSize::XSmall).color(Color::Error))
                    })
                    .when_some(current_thread, |this, thread| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Label::new("Thread")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .child(Label::new(thread).size(LabelSize::XSmall)),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("refresh", "Refresh")
                                    .style(ButtonStyle::Subtle)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.refresh(cx);
                                    })),
                            )
                            .when(
                                matches!(
                                    self.agent_status.as_ref().map(|s| &s.state),
                                    Some(AgentState::Working) | Some(AgentState::Idle)
                                ),
                                |this| {
                                    this.child(
                                        Button::new("pause", "Pause")
                                            .style(ButtonStyle::Subtle)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.set_state(AgentState::Blocked, cx);
                                            })),
                                    )
                                },
                            )
                            .when(
                                matches!(
                                    self.agent_status.as_ref().map(|s| &s.state),
                                    Some(AgentState::Blocked)
                                ),
                                |this| {
                                    this.child(
                                        Button::new("resume", "Resume")
                                            .style(ButtonStyle::Subtle)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.set_state(AgentState::Working, cx);
                                            })),
                                    )
                                },
                            )
                            .child(
                                Button::new("kill", "Kill Agent")
                                    .style(ButtonStyle::Tinted(TintColor::Error))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.kill_agent(cx);
                                    })),
                            ),
                    )
                    .when(!risks.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    Label::new("RISKS")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Warning),
                                )
                                .children(risks.into_iter().map(|r| {
                                    let severity_color = match r.severity {
                                        RiskSeverity::High => Color::Error,
                                        RiskSeverity::Medium => Color::Warning,
                                        RiskSeverity::Low => Color::Muted,
                                    };
                                    let badge = format!("[{}] {}", r.severity, r.status);
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            Label::new(badge)
                                                .size(LabelSize::XSmall)
                                                .color(severity_color),
                                        )
                                        .child(
                                            Label::new(r.title)
                                                .size(LabelSize::XSmall)
                                                .color(Color::Default),
                                        )
                                })),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                Label::new("SESSION HISTORY")
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
                                let findings_count = s.findings.len();
                                let files_count = s.files_touched.len();
                                v_flex()
                                    .gap_0p5()
                                    .p_1()
                                    .rounded_sm()
                                    .bg(cx.theme().colors().element_background)
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Label::new(
                                                    s.started_at
                                                        .format("%Y-%m-%d %H:%M")
                                                        .to_string(),
                                                )
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                            )
                                            .when(findings_count > 0, |this| {
                                                this.child(
                                                    Label::new(format!(
                                                        "{} findings",
                                                        findings_count
                                                    ))
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Accent),
                                                )
                                            })
                                            .when(files_count > 0, |this| {
                                                this.child(
                                                    Label::new(format!("{} files", files_count))
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Accent),
                                                )
                                            }),
                                    )
                                    .child(Label::new(summary).size(LabelSize::XSmall))
                            })),
                    )
                    // Live output section (only if spawned in this session)
                    .when_some(RunningAgents::global(cx), |this, running_agents| {
                        let ra = running_agents.read(cx);
                        let lines = ra.output_lines(&self.agent_name);
                        if !ra.was_spawned(&self.agent_name) || lines.is_empty() {
                            return this;
                        }
                        let heading = if ra.is_running(&self.agent_name) { "LIVE OUTPUT" } else { "OUTPUT (completed)" };
                        let mut output_view = v_flex().w_full().px_2().gap_0p5().child(
                            Label::new(heading)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        );
                        for line in lines.iter().rev().take(20).rev() {
                            output_view = output_view.child(
                                Label::new(line.clone())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Default),
                            );
                        }
                        this.child(output_view)
                    }),
            )
    }
}

/// Open or activate an AgentViewItem for the given agent name.
pub fn open_agent_view(
    workspace: &mut workspace::Workspace,
    agent_name: String,
    window: &mut Window,
    cx: &mut Context<workspace::Workspace>,
) {
    let existing = workspace.active_pane().read(cx).items().find_map(|item| {
        let agent_view = item.downcast::<AgentViewItem>()?;
        if agent_view.read(cx).agent_name == agent_name {
            Some(agent_view)
        } else {
            None
        }
    });

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let name = agent_name.clone();
        let ws_weak = cx.weak_entity();
        let item = cx.new(|cx: &mut Context<AgentViewItem>| {
            AgentViewItem::new(name, Some(ws_weak), window, cx)
        });
        workspace.add_item_to_center(Box::new(item), window, cx);
    }
}
