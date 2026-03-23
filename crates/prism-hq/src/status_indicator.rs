use std::sync::Arc;

use gpui::{Context, Render};
use prism_context::model::{AgentState, InboxEntryType, InboxSeverity};
use ui::{
    ButtonLike, Color, ContextMenu, ContextMenuEntry, Label, LabelSize, PopoverMenu,
    PopoverMenuHandle, prelude::*,
};
use uuid::Uuid;
use workspace::StatusItemView;

use crate::activity_bus;
use crate::context_service::ContextService;
use crate::hq_state::HqState;
use crate::manager_surface::OpenManagerSurface;

pub struct PrismStatusIndicator {
    popover_menu_handle: PopoverMenuHandle<ContextMenu>,
    _hq_subscription: Option<gpui::Subscription>,
    _activity_subscription: Option<gpui::Subscription>,
    /// (name, state, current_thread) — Arc for cheap clones in render
    agent_summaries: Arc<Vec<(String, AgentState, Option<String>)>>,
    actionable_count: usize,
    /// Unread inbox entries for popover display: (id, severity, type_label, title, source_agent)
    inbox_entries: Arc<Vec<(Uuid, InboxSeverity, String, String, Option<String>)>>,
    /// Count of High-severity unverified risks.
    high_risk_count: usize,
    /// Current mission phase label (e.g. "implement"), if an active plan exists.
    active_mission_phase: Option<String>,
    /// Cumulative cost USD from active plan sessions.
    cumulative_cost_usd: f64,
    /// Cached label — recomputed only when state changes
    label: String,
    /// Cached dot color — recomputed only when state changes
    dot_color: Color,
    /// Live activity from agent_ui (current tool/file)
    live_tool: Option<String>,
    live_file: Option<String>,
    is_generating: bool,
    waiting_for_approval: bool,
}

fn status_row(
    text: String,
) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyElement + 'static {
    move |_, _| {
        Label::new(text.clone())
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element()
    }
}

fn entry_type_label(t: &InboxEntryType) -> &'static str {
    match t {
        InboxEntryType::Approval => "Approval",
        InboxEntryType::Blocked => "Blocked",
        InboxEntryType::Suggestion => "Suggestion",
        InboxEntryType::Risk => "Risk",
        InboxEntryType::CostSpike => "Cost",
        InboxEntryType::Completed => "Done",
    }
}

fn severity_dot(s: &InboxSeverity) -> &'static str {
    match s {
        InboxSeverity::Critical => "⚠",
        InboxSeverity::Warning => "◆",
        InboxSeverity::Info => "·",
    }
}

impl PrismStatusIndicator {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let hq_subscription = HqState::global(cx).map(|hq_entity| {
            cx.observe(&hq_entity, |this, hq_entity, cx| {
                let hq = hq_entity.read(cx);
                let unread: Vec<_> = hq.inbox_entries.iter().filter(|e| !e.read).collect();
                let new_actionable = unread.len();
                let new_high_risks = hq.high_risk_count;
                let new_mission_phase = hq.active_plan().map(|p| p.current_phase.to_string());
                let new_cost = hq.cumulative_cost_usd;

                // Skip notify if nothing relevant changed.
                let unchanged = hq.agents.len() == this.agent_summaries.len()
                    && new_actionable == this.actionable_count
                    && new_high_risks == this.high_risk_count
                    && new_mission_phase == this.active_mission_phase
                    && (new_cost - this.cumulative_cost_usd).abs() < 0.001
                    && hq.agents
                        .iter()
                        .zip(this.agent_summaries.iter())
                        .all(|(a, (name, state, thread))| {
                            a.name == *name
                                && a.state == *state
                                && a.current_thread == *thread
                        });
                if unchanged {
                    return;
                }

                this.agent_summaries = Arc::new(
                    hq.agents
                        .iter()
                        .map(|a| (a.name.clone(), a.state.clone(), a.current_thread.clone()))
                        .collect::<Vec<_>>(),
                );
                this.inbox_entries = Arc::new(
                    unread
                        .into_iter()
                        .map(|e| {
                            (
                                e.id,
                                e.severity.clone(),
                                entry_type_label(&e.entry_type).to_string(),
                                e.title.clone(),
                                e.source_agent.clone(),
                            )
                        })
                        .collect(),
                );
                this.actionable_count = new_actionable;
                this.high_risk_count = new_high_risks;
                this.active_mission_phase = new_mission_phase;
                this.cumulative_cost_usd = new_cost;
                this.label = this.compute_label();
                this.dot_color = this.compute_dot_color();
                cx.notify();
            })
        });

        let activity_subscription =
            activity_bus::global_inner(cx).map(|bus_entity| {
                cx.observe(&bus_entity, |this, bus_entity, cx| {
                    let bus = bus_entity.read(cx);
                    let new_generating = bus.is_generating;
                    let new_waiting = bus.waiting_for_approval;
                    let new_tool = bus.current_tool.clone();
                    let new_file = bus.current_file.clone();

                    if this.is_generating == new_generating
                        && this.waiting_for_approval == new_waiting
                        && this.live_tool == new_tool
                        && this.live_file == new_file
                    {
                        return;
                    }

                    this.is_generating = new_generating;
                    this.waiting_for_approval = new_waiting;
                    this.live_tool = new_tool;
                    this.live_file = new_file;
                    this.label = this.compute_label();
                    this.dot_color = this.compute_dot_color();
                    cx.notify();
                })
            });

        Self {
            popover_menu_handle: PopoverMenuHandle::default(),
            _hq_subscription: hq_subscription,
            _activity_subscription: activity_subscription,
            agent_summaries: Arc::new(Vec::new()),
            actionable_count: 0,
            inbox_entries: Arc::new(Vec::new()),
            high_risk_count: 0,
            active_mission_phase: None,
            cumulative_cost_usd: 0.0,
            label: "P ● idle".to_string(),
            dot_color: Color::Success,
            live_tool: None,
            live_file: None,
            is_generating: false,
            waiting_for_approval: false,
        }
    }

    fn compute_label(&self) -> String {
        if self.waiting_for_approval {
            let mut s = "P ◐ awaiting approval".to_string();
            if let Some(phase) = &self.active_mission_phase {
                s.push_str(&format!(" · {phase}"));
            }
            return s;
        }
        if self.is_generating {
            if let Some(tool) = &self.live_tool {
                let base = format!("P ◐ {tool}");
                if let Some(file) = &self.live_file {
                    let short = std::path::Path::new(file.as_str())
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(file.as_str());
                    let mut s = format!("{base} {short}");
                    if let Some(phase) = &self.active_mission_phase {
                        s.push_str(&format!(" · {phase}"));
                    }
                    return s;
                }
                let mut s = base;
                if let Some(phase) = &self.active_mission_phase {
                    s.push_str(&format!(" · {phase}"));
                }
                return s;
            }
            let mut s = "P ◐ generating".to_string();
            if let Some(phase) = &self.active_mission_phase {
                s.push_str(&format!(" · {phase}"));
            }
            return s;
        }
        let agent_count = self.agent_summaries.len();
        if agent_count == 0 {
            if self.actionable_count > 0 {
                format!(
                    "P ● {} review{}",
                    self.actionable_count,
                    if self.actionable_count == 1 { "" } else { "s" }
                )
            } else if let Some(phase) = &self.active_mission_phase {
                format!("P ● idle · {phase}")
            } else {
                "P ● idle".to_string()
            }
        } else {
            let mut s = format!(
                "P ● {} agent{}",
                agent_count,
                if agent_count == 1 { "" } else { "s" }
            );
            if self.actionable_count > 0 {
                s.push_str(&format!(" · {} review", self.actionable_count));
            }
            if self.high_risk_count > 0 {
                s.push_str(&format!(" · {} risk", self.high_risk_count));
            }
            if let Some(phase) = &self.active_mission_phase {
                s.push_str(&format!(" · {phase}"));
            }
            s
        }
    }

    fn compute_dot_color(&self) -> Color {
        if self.agent_summaries.iter().any(|(_, state, _)| *state == AgentState::Blocked) {
            Color::Error
        } else if self.waiting_for_approval {
            Color::Warning
        } else if self.is_generating {
            Color::Accent
        } else if self.high_risk_count > 0 || self.actionable_count > 0 {
            Color::Warning
        } else {
            Color::Success
        }
    }
}

impl Render for PrismStatusIndicator {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut Context<Self>,
    ) -> impl gpui::IntoElement {
        let label = self.label.clone();
        let dot_color = self.dot_color;
        let agent_summaries = self.agent_summaries.clone();
        let inbox_entries = self.inbox_entries.clone();
        let actionable_count = self.actionable_count;
        let live_tool = self.live_tool.clone();
        let live_file = self.live_file.clone();
        let is_generating = self.is_generating;
        let waiting_for_approval = self.waiting_for_approval;
        let cost = self.cumulative_cost_usd;

        gpui::div().child(
            PopoverMenu::new("prism-status-popover")
                .anchor(gpui::Corner::BottomLeft)
                .menu(move |window, cx| {
                    let agent_summaries = agent_summaries.clone();
                    let inbox_entries = inbox_entries.clone();
                    let live_tool = live_tool.clone();
                    let live_file = live_file.clone();
                    Some(ContextMenu::build(window, cx, move |menu, _, _| {
                        // Cost header
                        let mut menu = if cost > 0.0 {
                            menu.header(format!("Session cost: ${cost:.2}"))
                        } else {
                            menu
                        };

                        // Current activity
                        menu = if is_generating || waiting_for_approval {
                            let activity_label = if waiting_for_approval {
                                "Waiting for tool approval".to_string()
                            } else if let Some(tool) = &live_tool {
                                if let Some(file) = &live_file {
                                    format!("{tool}: {file}")
                                } else {
                                    format!("Running {tool}…")
                                }
                            } else {
                                "Generating…".to_string()
                            };
                            menu.header("Current Activity")
                                .custom_row(status_row(activity_label))
                                .separator()
                                .header("Agents")
                        } else {
                            menu.header("Agents")
                        };

                        if agent_summaries.is_empty() {
                            menu = menu.custom_row(status_row("No active agents".to_string()));
                        } else {
                            for (name, state, thread) in agent_summaries.as_ref() {
                                let state_dot = match state {
                                    AgentState::Working => "●",
                                    AgentState::AwaitingReview => "◉",
                                    AgentState::Blocked => "⊗",
                                    _ => "○",
                                };
                                let row_label = if let Some(t) = thread {
                                    format!("{state_dot} {name}  [{t}]")
                                } else {
                                    format!("{state_dot} {name}  {state}")
                                };
                                menu = menu.custom_row(status_row(row_label));
                            }
                        }

                        // Review items — show each entry with dismiss action
                        if actionable_count > 0 {
                            menu = menu.separator().header(format!(
                                "Review ({actionable_count})"
                            ));

                            for (id, severity, type_label, title, source) in inbox_entries.as_ref() {
                                let dot = severity_dot(severity);
                                let prefix = if let Some(src) = source {
                                    format!("{dot} [{type_label}] {src}: ")
                                } else {
                                    format!("{dot} [{type_label}] ")
                                };
                                // Truncate title to keep menu readable
                                let short_title: String = title.chars().take(60).collect();
                                let row_text = format!("{prefix}{short_title}");
                                menu = menu.custom_row(status_row(row_text));

                                let dismiss_id = *id;
                                menu = menu.item(
                                    ContextMenuEntry::new(format!("  Dismiss ↑"))
                                        .handler(move |_window, cx| {
                                            if let Some(handle) = cx
                                                .try_global::<ContextService>()
                                                .and_then(|s| s.handle())
                                            {
                                                cx.background_spawn(async move {
                                                    let _ = handle.dismiss_inbox_entry(dismiss_id);
                                                })
                                                .detach();
                                            }
                                        }),
                                );
                            }

                            // Dismiss-all shortcut
                            let all_ids: Vec<Uuid> =
                                inbox_entries.iter().map(|(id, ..)| *id).collect();
                            if all_ids.len() > 1 {
                                menu = menu.item(
                                    ContextMenuEntry::new("Dismiss All")
                                        .handler(move |_window, cx| {
                                            if let Some(handle) = cx
                                                .try_global::<ContextService>()
                                                .and_then(|s| s.handle())
                                            {
                                                let ids = all_ids.clone();
                                                cx.background_spawn(async move {
                                                    for id in ids {
                                                        let _ = handle.dismiss_inbox_entry(id);
                                                    }
                                                })
                                                .detach();
                                            }
                                        }),
                                );
                            }
                        }

                        // Quick navigation to Manager Surface
                        menu = menu.separator().item(
                            ContextMenuEntry::new("Open Manager Surface")
                                .handler(|window, cx| {
                                    window.dispatch_action(Box::new(OpenManagerSurface), cx);
                                }),
                        );

                        menu
                    }))
                })
                .trigger(
                    ButtonLike::new("prism-status-trigger").child(
                        Label::new(label).size(LabelSize::Small).color(dot_color),
                    ),
                )
                .with_handle(self.popover_menu_handle.clone()),
        )
    }
}

impl StatusItemView for PrismStatusIndicator {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn workspace::item::ItemHandle>,
        _window: &mut gpui::Window,
        _cx: &mut Context<Self>,
    ) {
    }
}
