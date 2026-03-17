use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, Render, Task, WeakEntity,
    Window, actions, px,
};
use prism_context::model::{AgentStatus, Decision, Memory, Thread};
use prism_context::store::MemoryFilters;
use ui::{Button, ButtonStyle, Color, Icon, IconName, Label, LabelSize, prelude::*, v_flex, h_flex};
use workspace::dock::{DockPosition, Panel, PanelEvent};

use crate::activity_bus;
use crate::context_service::ContextService;
use crate::hq_state::HqState;

actions!(prism_hq, [ToggleContextPanel]);

const CONTEXT_PANEL_KEY: &str = "prism_context_panel";

pub struct ContextPanel {
    focus_handle: FocusHandle,
    _hq_subscription: Option<gpui::Subscription>,
    _activity_subscription: Option<gpui::Subscription>,
    position: DockPosition,
    width: Option<gpui::Pixels>,
    // Data
    memories: Vec<Memory>,
    decisions: Vec<Decision>,
    agents: Vec<AgentStatus>,
    threads: Vec<Thread>,
    current_thread_title: Option<String>,
    is_loading: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
}

impl EventEmitter<PanelEvent> for ContextPanel {}

impl ContextPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let auto_refresh = cx.spawn(async move |this: WeakEntity<ContextPanel>, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(10))
                    .await;
                this.update(cx, |panel, cx| panel.refresh(cx)).ok();
            }
        });

        let hq_subscription = HqState::global(cx).map(|hq_entity| {
            cx.observe(&hq_entity, |this, hq, cx| {
                let agents = hq.read(cx).agents.clone();
                this.agents = agents;
                cx.notify();
            })
        });

        let activity_subscription = activity_bus::global_inner(cx)
            .map(|bus_entity| {
                cx.observe(&bus_entity, |this, bus, cx| {
                    this.current_thread_title = bus.read(cx).thread_title.clone();
                    cx.notify();
                })
            });

        let mut panel = Self {
            focus_handle,
            _hq_subscription: hq_subscription,
            _activity_subscription: activity_subscription,
            position: DockPosition::Left,
            width: None,
            memories: Vec::new(),
            decisions: Vec::new(),
            agents: Vec::new(),
            threads: Vec::new(),
            current_thread_title: None,
            is_loading: false,
            error: None,
            refresh_task: None,
            _auto_refresh: auto_refresh,
        };
        panel.refresh(cx);
        panel
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.is_loading = true;
        cx.notify();

        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let handle = this
                .update(cx, |_, cx| {
                    cx.try_global::<ContextService>()
                        .and_then(|svc| svc.handle())
                })
                .ok()
                .flatten();

            let result: anyhow::Result<(Vec<Memory>, Vec<Decision>, Vec<Thread>)> = cx
                .background_spawn(async move {
                    let handle = handle
                        .ok_or_else(|| anyhow::anyhow!("context service not available"))?;
                    let memories = handle.load_memories(MemoryFilters::default())?;
                    let decisions = handle.list_decisions(None, None)?;
                    let threads = handle.list_threads(None)?;
                    anyhow::Ok((memories, decisions, threads))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok((memories, decisions, threads)) => {
                        this.memories = memories;
                        this.decisions = decisions;
                        this.threads = threads;
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
}

impl Focusable for ContextPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ContextPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let memories = self.memories.clone();
        let decisions = self.decisions.clone();
        let agents = self.agents.clone();
        let threads = self.threads.clone();
        let current_thread = self.current_thread_title.clone();
        let is_loading = self.is_loading;
        let error = self.error.clone();

        // Build content sections imperatively to avoid type inference issues in chained .when()
        let mut content = v_flex().flex_1().overflow_hidden().p_2().gap_3();

        if is_loading && memories.is_empty() {
            content = content.child(
                Label::new("Loading…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        }
        if let Some(err) = error {
            content = content.child(
                Label::new(format!("Error: {err}"))
                    .size(LabelSize::Small)
                    .color(Color::Error),
            );
        }

        // Active Thread section
        content = content.child(
            v_flex()
                .gap_1()
                .child(
                    Label::new("Active Thread")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    Label::new(current_thread.unwrap_or_else(|| "None".to_string()))
                        .size(LabelSize::Small),
                ),
        );

        // Threads section
        if !threads.is_empty() {
            content = content.child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new("Threads")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .children(threads.into_iter().take(5).map(|thread| {
                        h_flex()
                            .gap_1()
                            .child(
                                Icon::new(IconName::Thread)
                                    .size(ui::IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(Label::new(thread.name).size(LabelSize::Small))
                    })),
            );
        }

        // Memories section
        if !memories.is_empty() {
            content = content.child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new("Memories")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .children(memories.into_iter().take(10).map(|mem| {
                        v_flex()
                            .gap_px()
                            .child(
                                Label::new(mem.key)
                                    .size(LabelSize::Small)
                                    .color(Color::Default),
                            )
                            .child(
                                Label::new(mem.value.chars().take(80).collect::<String>())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                    })),
            );
        }

        // Decisions section
        if !decisions.is_empty() {
            content = content.child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new("Decisions")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .children(decisions.into_iter().take(5).map(|dec| {
                        v_flex()
                            .gap_px()
                            .child(
                                Label::new(dec.title)
                                    .size(LabelSize::Small)
                                    .color(Color::Default),
                            )
                            .child(
                                Label::new(dec.content.chars().take(80).collect::<String>())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                    })),
            );
        }

        // Agent Roster section
        if !agents.is_empty() {
            content = content.child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new("Agents")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .children(agents.into_iter().map(|agent| {
                        h_flex()
                            .gap_1()
                            .child(Label::new(agent.name).size(LabelSize::Small))
                            .child(
                                Label::new(agent.state.to_string())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                    })),
            );
        }

        v_flex()
            .size_full()
            .overflow_hidden()
            .child(
                // Header
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(Label::new("Context").size(LabelSize::Small).color(Color::Muted))
                    .child(gpui::div().flex_1())
                    .child(
                        Button::new("refresh-context", "↻")
                            .style(ButtonStyle::Transparent)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh(cx);
                            })),
                    ),
            )
            .child(content)
    }
}

impl Panel for ContextPanel {
    fn persistent_name() -> &'static str {
        "PrismContextPanel"
    }

    fn panel_key() -> &'static str {
        CONTEXT_PANEL_KEY
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _window: &mut Window, cx: &mut Context<Self>) {
        self.position = position;
        cx.notify();
    }

    fn size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        self.width.unwrap_or(px(280.0))
    }

    fn set_size(&mut self, size: Option<gpui::Pixels>, _window: &mut Window, cx: &mut Context<Self>) {
        self.width = size;
        cx.notify();
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Ai)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Prism Context")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleContextPanel)
    }

    fn activation_priority(&self) -> u32 {
        8
    }
}
