use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, Styled, Task, WeakEntity, Window, actions,
};
use uglyhat::model::{
    AgentSession, AgentState, AgentStatus, Decision, Handoff, HandoffConstraints, HandoffStatus,
    Memory, ThreadContext, ThreadStatus,
};
use ui::{
    Button, ButtonStyle, Color, Icon, IconName, Label, LabelSize, TintColor, h_flex, prelude::*,
    v_flex,
};
use workspace::item::{Item, ItemEvent};

use crate::inline_forms::{
    AddMemoryForm, CreateHandoffForm, DecisionField, MemoryField, RecordDecisionForm,
};
use uglyhat_panel::UglyhatService;

actions!(prism_hq, [OpenThreadView]);

/// Which inline form is currently open (mutually exclusive).
#[derive(Default, Clone, Copy, PartialEq)]
enum ActiveForm {
    #[default]
    None,
    AddMemory,
    RecordDecision,
    CreateHandoff,
}

pub struct ThreadViewItem {
    focus_handle: FocusHandle,
    thread_name: String,
    thread_context: Option<ThreadContext>,
    handoffs: Vec<Handoff>,
    assigned_agents: Vec<AgentStatus>,
    is_loading: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
    // Inline form state
    active_form: ActiveForm,
    add_memory: AddMemoryForm,
    record_decision: RecordDecisionForm,
    create_handoff: CreateHandoffForm,
    save_task: Option<Task<()>>,
}

impl ThreadViewItem {
    pub fn new(thread_name: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let auto_refresh = cx.spawn(async move |this: WeakEntity<ThreadViewItem>, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                this.update(cx, |item, cx| item.refresh(cx)).ok();
            }
        });

        let mut item = ThreadViewItem {
            focus_handle,
            thread_name,
            thread_context: None,
            handoffs: Vec::new(),
            assigned_agents: Vec::new(),
            is_loading: false,
            error: None,
            refresh_task: None,
            _auto_refresh: auto_refresh,
            active_form: ActiveForm::None,
            add_memory: AddMemoryForm::default(),
            record_decision: RecordDecisionForm::new(),
            create_handoff: CreateHandoffForm::new(),
            save_task: None,
        };
        item.refresh(cx);
        item
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.is_loading = true;
        cx.notify();

        let thread_name = self.thread_name.clone();

        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<UglyhatService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<(ThreadContext, Vec<Handoff>, Vec<AgentStatus>)> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("uglyhat not available"))?;
                    let ctx = handle.recall_thread(&thread_name)?;
                    let all_handoffs = handle.list_handoffs(None, None)?;
                    let handoffs: Vec<Handoff> = all_handoffs
                        .into_iter()
                        .filter(|h| h.thread_id == Some(ctx.thread.id))
                        .collect();
                    let all_agents = handle.list_agents()?;
                    let assigned: Vec<AgentStatus> = all_agents
                        .into_iter()
                        .filter(|a| a.current_thread.as_deref() == Some(&thread_name))
                        .collect();
                    anyhow::Ok((ctx, handoffs, assigned))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                let changed = match result {
                    Ok((ctx, handoffs, assigned)) => {
                        let changed = this.error.is_some()
                            || this.thread_context.as_ref().map(|c| c.thread.id)
                                != Some(ctx.thread.id)
                            || this.handoffs.len() != handoffs.len()
                            || this.assigned_agents.len() != assigned.len();
                        this.thread_context = Some(ctx);
                        this.handoffs = handoffs;
                        this.assigned_agents = assigned;
                        this.error = None;
                        changed
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        let changed = this.error.as_deref() != Some(msg.as_str());
                        this.error = Some(msg);
                        changed
                    }
                };
                if changed {
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn save_memory(&mut self, cx: &mut Context<Self>) {
        let key = self.add_memory.key_input.trim().to_string();
        let value = self.add_memory.value_input.trim().to_string();
        if key.is_empty() || value.is_empty() {
            return;
        }

        self.add_memory.saving = true;
        cx.notify();

        let thread_name = self.thread_name.clone();

        self.save_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<UglyhatService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<()> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("uglyhat not available"))?;
                    handle.save_memory(&key, &value, Some(&thread_name), vec![])?;
                    anyhow::Ok(())
                })
                .await;

            this.update(cx, |this, cx| {
                this.add_memory.saving = false;
                if let Err(e) = result {
                    this.error = Some(e.to_string());
                } else {
                    this.add_memory.key_input.clear();
                    this.add_memory.value_input.clear();
                    this.active_form = ActiveForm::None;
                    this.refresh(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn save_decision(&mut self, cx: &mut Context<Self>) {
        let title = self.record_decision.title_input.trim().to_string();
        let content = self.record_decision.content_input.trim().to_string();
        if title.is_empty() {
            return;
        }

        self.record_decision.saving = true;
        cx.notify();

        let thread_id = self.thread_context.as_ref().map(|ctx| ctx.thread.id);
        let scope = self.record_decision.scope.clone();

        self.save_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<UglyhatService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<()> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("uglyhat not available"))?;
                    handle.save_decision(&title, &content, thread_id, vec![], scope)?;
                    anyhow::Ok(())
                })
                .await;

            this.update(cx, |this, cx| {
                this.record_decision.saving = false;
                if let Err(e) = result {
                    this.error = Some(e.to_string());
                } else {
                    this.record_decision.title_input.clear();
                    this.record_decision.content_input.clear();
                    this.active_form = ActiveForm::None;
                    this.refresh(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn save_handoff(&mut self, cx: &mut Context<Self>) {
        let task = self.create_handoff.task_input.trim().to_string();
        if task.is_empty() {
            return;
        }

        self.create_handoff.saving = true;
        cx.notify();

        let thread_id = self.thread_context.as_ref().map(|ctx| ctx.thread.id);
        let mode = self.create_handoff.mode.clone();

        self.save_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<UglyhatService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<()> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("uglyhat not available"))?;
                    handle.create_handoff(
                        "zed-user",
                        &task,
                        thread_id,
                        HandoffConstraints::default(),
                        mode,
                    )?;
                    anyhow::Ok(())
                })
                .await;

            this.update(cx, |this, cx| {
                this.create_handoff.saving = false;
                if let Err(e) = result {
                    this.error = Some(e.to_string());
                } else {
                    this.create_handoff.task_input.clear();
                    this.active_form = ActiveForm::None;
                    this.refresh(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_agent_state_inline(
        &mut self,
        agent_name: String,
        state: AgentState,
        cx: &mut Context<Self>,
    ) {
        self.save_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<UglyhatService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<()> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("uglyhat not available"))?;
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
}

impl EventEmitter<ItemEvent> for ThreadViewItem {}

impl Focusable for ThreadViewItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for ThreadViewItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!("Thread: {}", self.thread_name).into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<ui::Icon> {
        Some(Icon::new(IconName::ListTodo))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

impl Render for ThreadViewItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let thread_name: SharedString = self.thread_name.clone().into();
        let is_loading = self.is_loading;
        let error = self.error.clone();
        let thread_context = self.thread_context.clone();
        let handoffs = self.handoffs.clone();
        let assigned_agents = self.assigned_agents.clone();
        let active_form = self.active_form;

        // Per-instance element IDs to avoid collisions when multiple ThreadViewItems are open.
        let mem_key_id = SharedString::from(format!("mem-key-{}", thread_name));
        let mem_val_id = SharedString::from(format!("mem-val-{}", thread_name));
        let dec_title_id = SharedString::from(format!("dec-title-{}", thread_name));
        let dec_content_id = SharedString::from(format!("dec-content-{}", thread_name));

        // Key handler for active inline forms.
        let key_handler = cx.listener(|this, event: &KeyDownEvent, _, cx| {
            if this.active_form == ActiveForm::None {
                return;
            }
            let ks = &event.keystroke;
            if ks.key == "escape" {
                this.active_form = ActiveForm::None;
                cx.notify();
                return;
            }
            if ks.key == "backspace" {
                match this.active_form {
                    ActiveForm::AddMemory => match this.add_memory.active_field {
                        MemoryField::Key => {
                            this.add_memory.key_input.pop();
                        }
                        MemoryField::Value => {
                            this.add_memory.value_input.pop();
                        }
                    },
                    ActiveForm::RecordDecision => match this.record_decision.active_field {
                        DecisionField::Title => {
                            this.record_decision.title_input.pop();
                        }
                        DecisionField::Content => {
                            this.record_decision.content_input.pop();
                        }
                    },
                    ActiveForm::CreateHandoff => {
                        this.create_handoff.task_input.pop();
                    }
                    ActiveForm::None => {}
                }
                cx.notify();
                return;
            }
            if ks.key == "tab" {
                match this.active_form {
                    ActiveForm::AddMemory => {
                        this.add_memory.active_field = match this.add_memory.active_field {
                            MemoryField::Key => MemoryField::Value,
                            MemoryField::Value => MemoryField::Key,
                        };
                    }
                    ActiveForm::RecordDecision => {
                        this.record_decision.active_field = match this.record_decision.active_field
                        {
                            DecisionField::Title => DecisionField::Content,
                            DecisionField::Content => DecisionField::Title,
                        };
                    }
                    _ => {}
                }
                cx.notify();
                return;
            }
            if !ks.modifiers.platform && !ks.modifiers.control && !ks.modifiers.alt {
                if let Some(ch) = &ks.key_char {
                    match this.active_form {
                        ActiveForm::AddMemory => match this.add_memory.active_field {
                            MemoryField::Key => this.add_memory.key_input.push_str(ch),
                            MemoryField::Value => this.add_memory.value_input.push_str(ch),
                        },
                        ActiveForm::RecordDecision => match this.record_decision.active_field {
                            DecisionField::Title => this.record_decision.title_input.push_str(ch),
                            DecisionField::Content => {
                                this.record_decision.content_input.push_str(ch)
                            }
                        },
                        ActiveForm::CreateHandoff => {
                            this.create_handoff.task_input.push_str(ch);
                        }
                        ActiveForm::None => {}
                    }
                    cx.notify();
                }
            }
        });

        v_flex()
            .key_context("ThreadView")
            .track_focus(&self.focus_handle)
            .on_key_down(key_handler)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            // Header bar
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .h(px(40.))
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .gap_2()
                    .child(Label::new(thread_name).size(LabelSize::Small))
                    .when_some(
                        thread_context.as_ref().map(|c| c.thread.status.clone()),
                        |this, status| {
                            let (label, color) = match status {
                                ThreadStatus::Active => ("active", Color::Success),
                                ThreadStatus::Archived => ("archived", Color::Muted),
                            };
                            this.child(Label::new(label).size(LabelSize::XSmall).color(color))
                        },
                    )
                    .flex_1()
                    .child(
                        Button::new("archive", "Archive")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::XSmall)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let thread_name = this.thread_name.clone();
                                this.save_task = Some(cx.spawn(async move |this_weak, cx| {
                                    let handle = this_weak
                                        .update(cx, |_, cx| {
                                            cx.try_global::<UglyhatService>()
                                                .and_then(|svc| svc.handle())
                                        })
                                        .ok()
                                        .flatten();
                                    let _result: anyhow::Result<()> = cx
                                        .background_spawn(async move {
                                            let handle = handle.ok_or_else(|| {
                                                anyhow::anyhow!("uglyhat not available")
                                            })?;
                                            handle.archive_thread(&thread_name)?;
                                            anyhow::Ok(())
                                        })
                                        .await;
                                    this_weak.update(cx, |this, cx| this.refresh(cx)).ok();
                                }));
                            })),
                    ),
            )
            // Main scrollable content
            .child(
                v_flex()
                    .id("thread-content")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .py_1()
                    .gap_3()
                    .when(is_loading && thread_context.is_none(), |this| {
                        this.child(
                            Label::new("Loading…")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .when_some(error, |this, err| {
                        this.child(Label::new(err).size(LabelSize::XSmall).color(Color::Error))
                    })
                    // Assigned Agents section
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                Label::new("ASSIGNED AGENTS")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .when(assigned_agents.is_empty(), |this| {
                                this.child(
                                    Label::new("No agents assigned")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            })
                            .children(assigned_agents.into_iter().enumerate().map(
                                |(ix, agent)| {
                                    let state_color = match agent.state {
                                        AgentState::Working => Color::Accent,
                                        AgentState::Idle => Color::Success,
                                        AgentState::Blocked => Color::Warning,
                                        AgentState::Dead => Color::Muted,
                                    };
                                    let agent_name_pause = agent.name.clone();
                                    let agent_name_kill = agent.name.clone();
                                    let can_pause = matches!(
                                        agent.state,
                                        AgentState::Working | AgentState::Idle
                                    );
                                    let can_resume = matches!(agent.state, AgentState::Blocked);
                                    h_flex()
                                        .id(("assigned-agent", ix))
                                        .w_full()
                                        .gap_1()
                                        .p_1()
                                        .rounded_sm()
                                        .bg(cx.theme().colors().element_background)
                                        .child(
                                            Label::new("●")
                                                .size(LabelSize::XSmall)
                                                .color(state_color),
                                        )
                                        .child(
                                            Label::new(agent.name.clone()).size(LabelSize::Small),
                                        )
                                        .flex_1()
                                        .when(can_pause, |this| {
                                            let name = agent_name_pause.clone();
                                            this.child(
                                                Button::new(("pause", ix), "Pause")
                                                    .style(ButtonStyle::Subtle)
                                                    .label_size(LabelSize::XSmall)
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.set_agent_state_inline(
                                                                name.clone(),
                                                                AgentState::Blocked,
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                            )
                                        })
                                        .when(can_resume, |this| {
                                            let name = agent_name_pause.clone();
                                            this.child(
                                                Button::new(("resume", ix), "Resume")
                                                    .style(ButtonStyle::Subtle)
                                                    .label_size(LabelSize::XSmall)
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.set_agent_state_inline(
                                                                name.clone(),
                                                                AgentState::Working,
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                            )
                                        })
                                        .child(
                                            Button::new(("kill-agent", ix), "Kill")
                                                .style(ButtonStyle::Tinted(TintColor::Error))
                                                .label_size(LabelSize::XSmall)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.set_agent_state_inline(
                                                        agent_name_kill.clone(),
                                                        AgentState::Dead,
                                                        cx,
                                                    );
                                                })),
                                        )
                                },
                            )),
                    )
                    // Activity Feed
                    .when_some(
                        thread_context.as_ref().map(|c| c.recent_activity.clone()),
                        |this, activity| {
                            this.child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        Label::new("RECENT ACTIVITY")
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .when(activity.is_empty(), |this| {
                                        this.child(
                                            Label::new("No recent activity")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                    })
                                    .children(activity.into_iter().map(|entry| {
                                        let summary = if !entry.summary.is_empty() {
                                            entry.summary.clone()
                                        } else {
                                            format!("{} {}", entry.action, entry.entity_type)
                                        };
                                        h_flex()
                                            .gap_1()
                                            .when(!entry.actor.is_empty(), |this| {
                                                this.child(
                                                    Label::new(entry.actor)
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Accent),
                                                )
                                            })
                                            .child(Label::new(summary).size(LabelSize::XSmall))
                                    })),
                            )
                        },
                    )
                    // Memories section
                    .when_some(
                        thread_context.as_ref().map(|c| c.memories.clone()),
                        |this, memories| {
                            let show_memory_form = active_form == ActiveForm::AddMemory;
                            this.child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Label::new("MEMORIES")
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Muted),
                                            )
                                            .flex_1()
                                            .child(
                                                Button::new("add-memory", "+ Add Memory")
                                                    .style(ButtonStyle::Subtle)
                                                    .label_size(LabelSize::XSmall)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.active_form = ActiveForm::AddMemory;
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                    .when(memories.is_empty() && !show_memory_form, |this| {
                                        this.child(
                                            Label::new("No memories")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                    })
                                    .children(memories.into_iter().map(|m: Memory| {
                                        v_flex()
                                            .gap_0p5()
                                            .p_1()
                                            .rounded_sm()
                                            .bg(cx.theme().colors().element_background)
                                            .child(
                                                Label::new(m.key.clone())
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Accent),
                                            )
                                            .child(Label::new(m.value).size(LabelSize::XSmall))
                                    }))
                                    .when(show_memory_form, |this| {
                                        let key_text = self.add_memory.key_input.clone();
                                        let val_text = self.add_memory.value_input.clone();
                                        let saving = self.add_memory.saving;
                                        this.child(
                                            v_flex()
                                                .gap_1()
                                                .p_1()
                                                .border_1()
                                                .border_color(cx.theme().colors().border_focused)
                                                .rounded_sm()
                                                .child(
                                                    h_flex()
                                                        .gap_1()
                                                        .child(
                                                            Label::new("Key:")
                                                                .size(LabelSize::XSmall)
                                                                .color(Color::Muted),
                                                        )
                                                        .child(
                                                            div()
                                                                .id(mem_key_id.clone())
                                                                .flex_1()
                                                                .px_1()
                                                                .border_1()
                                                                .border_color(
                                                                    if self.add_memory.active_field
                                                                        == MemoryField::Key
                                                                    {
                                                                        cx.theme()
                                                                            .colors()
                                                                            .border_focused
                                                                    } else {
                                                                        cx.theme().colors().border
                                                                    },
                                                                )
                                                                .rounded_sm()
                                                                .cursor_text()
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.add_memory
                                                                            .active_field =
                                                                            MemoryField::Key;
                                                                        cx.notify();
                                                                    },
                                                                ))
                                                                .child(if key_text.is_empty() {
                                                                    Label::new("key")
                                                                        .size(LabelSize::XSmall)
                                                                        .color(Color::Muted)
                                                                        .into_any_element()
                                                                } else {
                                                                    Label::new(key_text)
                                                                        .size(LabelSize::XSmall)
                                                                        .into_any_element()
                                                                }),
                                                        ),
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap_1()
                                                        .child(
                                                            Label::new("Value:")
                                                                .size(LabelSize::XSmall)
                                                                .color(Color::Muted),
                                                        )
                                                        .child(
                                                            div()
                                                                .id(mem_val_id.clone())
                                                                .flex_1()
                                                                .px_1()
                                                                .border_1()
                                                                .border_color(
                                                                    if self.add_memory.active_field
                                                                        == MemoryField::Value
                                                                    {
                                                                        cx.theme()
                                                                            .colors()
                                                                            .border_focused
                                                                    } else {
                                                                        cx.theme().colors().border
                                                                    },
                                                                )
                                                                .rounded_sm()
                                                                .cursor_text()
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.add_memory
                                                                            .active_field =
                                                                            MemoryField::Value;
                                                                        cx.notify();
                                                                    },
                                                                ))
                                                                .child(if val_text.is_empty() {
                                                                    Label::new("value")
                                                                        .size(LabelSize::XSmall)
                                                                        .color(Color::Muted)
                                                                        .into_any_element()
                                                                } else {
                                                                    Label::new(val_text)
                                                                        .size(LabelSize::XSmall)
                                                                        .into_any_element()
                                                                }),
                                                        ),
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap_1()
                                                        .child(
                                                            Button::new(
                                                                "save-memory",
                                                                if saving {
                                                                    "Saving…"
                                                                } else {
                                                                    "Save"
                                                                },
                                                            )
                                                            .style(ButtonStyle::Filled)
                                                            .label_size(LabelSize::XSmall)
                                                            .disabled(saving)
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.save_memory(cx);
                                                                },
                                                            )),
                                                        )
                                                        .child(
                                                            Button::new("cancel-memory", "Cancel")
                                                                .style(ButtonStyle::Subtle)
                                                                .label_size(LabelSize::XSmall)
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.active_form =
                                                                            ActiveForm::None;
                                                                        cx.notify();
                                                                    },
                                                                )),
                                                        ),
                                                ),
                                        )
                                    }),
                            )
                        },
                    )
                    // Decisions section
                    .when_some(
                        thread_context.as_ref().map(|c| c.decisions.clone()),
                        |this, decisions| {
                            let show_decision_form = active_form == ActiveForm::RecordDecision;
                            this.child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Label::new("DECISIONS")
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Muted),
                                            )
                                            .flex_1()
                                            .child(
                                                Button::new("add-decision", "+ Record Decision")
                                                    .style(ButtonStyle::Subtle)
                                                    .label_size(LabelSize::XSmall)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.active_form =
                                                            ActiveForm::RecordDecision;
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                    .when(decisions.is_empty() && !show_decision_form, |this| {
                                        this.child(
                                            Label::new("No decisions")
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                    })
                                    .children(decisions.into_iter().map(|d: Decision| {
                                        v_flex()
                                            .gap_0p5()
                                            .p_1()
                                            .rounded_sm()
                                            .bg(cx.theme().colors().element_background)
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
                                    }))
                                    .when(show_decision_form, |this| {
                                        let title_text = self.record_decision.title_input.clone();
                                        let content_text =
                                            self.record_decision.content_input.clone();
                                        let saving = self.record_decision.saving;
                                        this.child(
                                            v_flex()
                                                .gap_1()
                                                .p_1()
                                                .border_1()
                                                .border_color(cx.theme().colors().border_focused)
                                                .rounded_sm()
                                                .child(
                                                    div()
                                                        .id(dec_title_id.clone())
                                                        .px_1()
                                                        .border_1()
                                                        .border_color(
                                                            if self.record_decision.active_field
                                                                == DecisionField::Title
                                                            {
                                                                cx.theme().colors().border_focused
                                                            } else {
                                                                cx.theme().colors().border
                                                            },
                                                        )
                                                        .rounded_sm()
                                                        .cursor_text()
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.record_decision.active_field =
                                                                DecisionField::Title;
                                                            cx.notify();
                                                        }))
                                                        .child(if title_text.is_empty() {
                                                            Label::new("Decision title…")
                                                                .size(LabelSize::XSmall)
                                                                .color(Color::Muted)
                                                                .into_any_element()
                                                        } else {
                                                            Label::new(title_text)
                                                                .size(LabelSize::XSmall)
                                                                .into_any_element()
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .id(dec_content_id.clone())
                                                        .px_1()
                                                        .min_h(px(40.))
                                                        .border_1()
                                                        .border_color(
                                                            if self.record_decision.active_field
                                                                == DecisionField::Content
                                                            {
                                                                cx.theme().colors().border_focused
                                                            } else {
                                                                cx.theme().colors().border
                                                            },
                                                        )
                                                        .rounded_sm()
                                                        .cursor_text()
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.record_decision.active_field =
                                                                DecisionField::Content;
                                                            cx.notify();
                                                        }))
                                                        .child(if content_text.is_empty() {
                                                            Label::new(
                                                                "Rationale (why this decision)…",
                                                            )
                                                            .size(LabelSize::XSmall)
                                                            .color(Color::Muted)
                                                            .into_any_element()
                                                        } else {
                                                            Label::new(content_text)
                                                                .size(LabelSize::XSmall)
                                                                .into_any_element()
                                                        }),
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap_1()
                                                        .child(
                                                            Button::new(
                                                                "save-decision",
                                                                if saving {
                                                                    "Saving…"
                                                                } else {
                                                                    "Save"
                                                                },
                                                            )
                                                            .style(ButtonStyle::Filled)
                                                            .label_size(LabelSize::XSmall)
                                                            .disabled(saving)
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.save_decision(cx);
                                                                },
                                                            )),
                                                        )
                                                        .child(
                                                            Button::new(
                                                                "cancel-decision",
                                                                "Cancel",
                                                            )
                                                            .style(ButtonStyle::Subtle)
                                                            .label_size(LabelSize::XSmall)
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.active_form =
                                                                        ActiveForm::None;
                                                                    cx.notify();
                                                                },
                                                            )),
                                                        ),
                                                ),
                                        )
                                    }),
                            )
                        },
                    )
                    // Handoffs section
                    .child({
                        let show_handoff_form = active_form == ActiveForm::CreateHandoff;
                        v_flex()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Label::new("HANDOFFS")
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .flex_1()
                                    .child(
                                        Button::new("add-handoff", "+ Create Handoff")
                                            .style(ButtonStyle::Subtle)
                                            .label_size(LabelSize::XSmall)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.active_form = ActiveForm::CreateHandoff;
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .when(handoffs.is_empty() && !show_handoff_form, |this| {
                                this.child(
                                    Label::new("No handoffs")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            })
                            .children(handoffs.iter().map(|h| {
                                let status_label = match h.status {
                                    HandoffStatus::Pending => ("pending", Color::Muted),
                                    HandoffStatus::Accepted => ("accepted", Color::Accent),
                                    HandoffStatus::Running => ("running", Color::Success),
                                    HandoffStatus::Completed => ("done", Color::Muted),
                                    HandoffStatus::Failed => ("failed", Color::Error),
                                    HandoffStatus::Cancelled => ("cancelled", Color::Muted),
                                };
                                h_flex()
                                    .gap_1()
                                    .p_1()
                                    .rounded_sm()
                                    .bg(cx.theme().colors().element_background)
                                    .child(
                                        Label::new(status_label.0)
                                            .size(LabelSize::XSmall)
                                            .color(status_label.1),
                                    )
                                    .child(Label::new(h.task.clone()).size(LabelSize::XSmall))
                            }))
                            .when(show_handoff_form, |this| {
                                let task_text = self.create_handoff.task_input.clone();
                                let saving = self.create_handoff.saving;
                                this.child(
                                    v_flex()
                                        .gap_1()
                                        .p_1()
                                        .border_1()
                                        .border_color(cx.theme().colors().border_focused)
                                        .rounded_sm()
                                        .child(
                                            div()
                                                .px_1()
                                                .min_h(px(32.))
                                                .border_1()
                                                .border_color(cx.theme().colors().border_focused)
                                                .rounded_sm()
                                                .cursor_text()
                                                .child(if task_text.is_empty() {
                                                    Label::new("Describe the task to hand off…")
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Muted)
                                                        .into_any_element()
                                                } else {
                                                    Label::new(task_text)
                                                        .size(LabelSize::XSmall)
                                                        .into_any_element()
                                                }),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(
                                                    Button::new(
                                                        "save-handoff",
                                                        if saving { "Saving…" } else { "Save" },
                                                    )
                                                    .style(ButtonStyle::Filled)
                                                    .label_size(LabelSize::XSmall)
                                                    .disabled(saving)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.save_handoff(cx);
                                                    })),
                                                )
                                                .child(
                                                    Button::new("cancel-handoff", "Cancel")
                                                        .style(ButtonStyle::Subtle)
                                                        .label_size(LabelSize::XSmall)
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.active_form = ActiveForm::None;
                                                            cx.notify();
                                                        })),
                                                ),
                                        ),
                                )
                            })
                    })
                    // Sessions section
                    .when_some(
                        thread_context.as_ref().map(|c| c.recent_sessions.clone()),
                        |this, sessions| {
                            this.child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        Label::new("SESSIONS")
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
                                                    .when(!s.findings.is_empty(), |this| {
                                                        this.child(
                                                            Label::new(format!(
                                                                "{} findings",
                                                                s.findings.len()
                                                            ))
                                                            .size(LabelSize::XSmall)
                                                            .color(Color::Accent),
                                                        )
                                                    })
                                                    .when(!s.files_touched.is_empty(), |this| {
                                                        this.child(
                                                            Label::new(format!(
                                                                "{} files",
                                                                s.files_touched.len()
                                                            ))
                                                            .size(LabelSize::XSmall)
                                                            .color(Color::Accent),
                                                        )
                                                    }),
                                            )
                                            .child(Label::new(summary).size(LabelSize::XSmall))
                                    })),
                            )
                        },
                    ),
            )
    }
}

/// Open or activate a ThreadViewItem for the given thread name.
pub fn open_thread_view(
    workspace: &mut workspace::Workspace,
    thread_name: String,
    window: &mut Window,
    cx: &mut Context<workspace::Workspace>,
) {
    let existing = workspace.active_pane().read(cx).items().find_map(|item| {
        let tv = item.downcast::<ThreadViewItem>()?;
        if tv.read(cx).thread_name == thread_name {
            Some(tv)
        } else {
            None
        }
    });

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let item =
            cx.new(|cx: &mut Context<ThreadViewItem>| ThreadViewItem::new(thread_name, window, cx));
        workspace.add_item_to_center(Box::new(item), window, cx);
    }
}
