use crate::types::DashboardData;
use anyhow::Result;
use db::kvp::KEY_VALUE_STORE;
use gpui::{
    actions, px, Action, App, AsyncWindowContext, ClickEvent, Context, ElementId, Entity,
    EventEmitter, FocusHandle, Focusable, IntoElement, ParentElement, Pixels, Render, Styled, Task,
    WeakEntity, Window,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use ui::{
    h_flex, prelude::*, v_flex, Color, Icon, IconButton, IconName, Label, LabelSize, Tooltip,
};
use util::{ResultExt, TryFutureExt};
use workspace::{
    dock::{DockPosition, Panel, PanelEvent},
    Workspace,
};

const PANEL_KEY: &str = "PrismDashboardPanel";
const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

actions!(
    prism_dashboard,
    [
        /// Toggles the PrisM dashboard panel.
        Toggle,
        /// Toggles focus on the PrisM dashboard panel.
        ToggleFocus
    ]
);

pub struct PrismDashboardPanel {
    focus_handle: FocusHandle,
    width: Option<Pixels>,
    active: bool,
    data: DashboardData,
    is_loading: bool,
    error: Option<String>,
    gateway_url: Option<String>,
    api_key: Option<String>,
    cost_expanded: bool,
    models_expanded: bool,
    waste_expanded: bool,
    routing_expanded: bool,
    agents_expanded: bool,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
    pending_serialization: Task<Option<()>>,
}

#[derive(Serialize, Deserialize)]
struct SerializedPanel {
    width: Option<Pixels>,
    #[serde(default = "default_true")]
    cost_expanded: bool,
    #[serde(default = "default_true")]
    models_expanded: bool,
    #[serde(default = "default_true")]
    waste_expanded: bool,
    #[serde(default = "default_true")]
    routing_expanded: bool,
    #[serde(default = "default_true")]
    agents_expanded: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug)]
pub enum Event {
    DockPositionChanged,
    Focus,
    Dismissed,
}

impl PrismDashboardPanel {
    pub fn new(
        _workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let gateway_url = std::env::var("PRISM_GATEWAY_URL").ok();
            let api_key = std::env::var("PRISM_API_KEY").ok();

            let mut panel = Self {
                focus_handle: cx.focus_handle(),
                width: None,
                active: false,
                data: DashboardData::default(),
                is_loading: false,
                error: None,
                gateway_url,
                api_key,
                cost_expanded: true,
                models_expanded: true,
                waste_expanded: true,
                routing_expanded: true,
                agents_expanded: true,
                refresh_task: None,
                _auto_refresh: Task::ready(()),
                pending_serialization: Task::ready(None),
            };

            let auto_refresh = cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(AUTO_REFRESH_INTERVAL).await;
                this.update(cx, |panel: &mut PrismDashboardPanel, cx| panel.refresh(cx))
                    .ok();
            });
            panel._auto_refresh = auto_refresh;

            panel.refresh(cx);
            panel
        })
    }

    pub fn load(
        workspace: WeakEntity<Workspace>,
        cx: AsyncWindowContext,
    ) -> Task<Result<Entity<Self>>> {
        cx.spawn(async move |cx| {
            let serialized_panel = cx
                .background_spawn(async move { KEY_VALUE_STORE.read_kvp(PANEL_KEY) })
                .await
                .log_err()
                .flatten()
                .and_then(|s| serde_json::from_str::<SerializedPanel>(&s).log_err());

            workspace.update_in(cx, |workspace, window, cx| {
                let panel = Self::new(workspace, window, cx);
                if let Some(serialized) = serialized_panel {
                    panel.update(cx, |panel, cx| {
                        panel.width = serialized.width.map(|w| w.round());
                        panel.cost_expanded = serialized.cost_expanded;
                        panel.models_expanded = serialized.models_expanded;
                        panel.waste_expanded = serialized.waste_expanded;
                        panel.routing_expanded = serialized.routing_expanded;
                        panel.agents_expanded = serialized.agents_expanded;
                        cx.notify();
                    });
                }
                panel
            })
        })
    }

    fn serialize(&mut self, cx: &mut Context<Self>) {
        let width = self.width;
        let cost_expanded = self.cost_expanded;
        let models_expanded = self.models_expanded;
        let waste_expanded = self.waste_expanded;
        let routing_expanded = self.routing_expanded;
        let agents_expanded = self.agents_expanded;
        self.pending_serialization = cx.background_spawn(
            async move {
                KEY_VALUE_STORE
                    .write_kvp(
                        PANEL_KEY.into(),
                        serde_json::to_string(&SerializedPanel {
                            width,
                            cost_expanded,
                            models_expanded,
                            waste_expanded,
                            routing_expanded,
                            agents_expanded,
                        })?,
                    )
                    .await?;
                anyhow::Ok(())
            }
            .log_err(),
        );
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(gateway_url) = self.gateway_url.clone() else {
            self.error = Some("Set PRISM_GATEWAY_URL to connect".into());
            cx.notify();
            return;
        };

        self.is_loading = true;
        self.error = None;
        cx.notify();

        let api_key = self.api_key.clone();

        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let result: anyhow::Result<_> = cx
                .background_spawn(async move {
                    let mut client = prism_client::PrismClient::new(&gateway_url);
                    if let Some(key) = api_key {
                        client = client.with_api_key(key);
                    }
                    let (summary, waste, task_types, policy, agents) = futures::join!(
                        client.stats_summary(7),
                        client.stats_waste_score(7),
                        client.stats_task_types(7),
                        client.routing_policy(),
                        client.stats_agents(7)
                    );
                    anyhow::Ok::<(_, _, _, _, _)>((summary, waste, task_types, policy, agents))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok((summary, waste, task_types, policy, agents)) => {
                        this.data.summary = summary.ok();
                        this.data.waste_score = waste.ok();
                        this.data.task_types = task_types.ok();
                        this.data.policy = policy.ok();
                        this.data.agent_metrics = agents.ok();
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

    fn render_section_header(
        id: impl Into<ElementId>,
        label: &str,
        expanded: bool,
        on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .id(id)
            .w_full()
            .px_2()
            .py_1()
            .gap_1()
            .bg(cx.theme().colors().surface_background)
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .cursor_pointer()
            .on_click(on_toggle)
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(ui::IconSize::Small)
                .color(Color::Muted),
            )
            .child(Label::new(label.to_owned()).size(LabelSize::Small))
    }

    fn render_cost_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cost_expanded = self.cost_expanded;
        v_flex()
            .w_full()
            .child(Self::render_section_header(
                "section-cost",
                "Cost Overview",
                cost_expanded,
                cx.listener(|this, _, _, cx| {
                    this.cost_expanded = !this.cost_expanded;
                    this.serialize(cx);
                    cx.notify();
                }),
                cx,
            ))
            .when(cost_expanded, |this| {
                if let Some(summary) = &self.data.summary {
                    this.child(
                        v_flex()
                            .w_full()
                            .px_3()
                            .pb_1()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .child(
                                        Label::new("Total")
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        Label::new(format!("${:.2}", summary.total_cost_usd))
                                            .size(LabelSize::Small),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .child(
                                        Label::new("Requests")
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        Label::new(format_number(summary.total_requests))
                                            .size(LabelSize::Small),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .child(
                                        Label::new("Tokens")
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        Label::new(format_tokens(summary.total_tokens))
                                            .size(LabelSize::Small),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .child(
                                        Label::new("Failures")
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        Label::new(format!("{:.1}%", summary.failure_rate * 100.0))
                                            .size(LabelSize::Small),
                                    ),
                            )
                            .child(
                                Label::new(format!("({}d)", summary.period_days))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                } else {
                    this.child(
                        div().px_3().pb_1().child(
                            Label::new("No data")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                }
            })
    }

    fn render_models_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let models_expanded = self.models_expanded;
        v_flex()
            .w_full()
            .child(Self::render_section_header(
                "section-models",
                "Per-Model Cost",
                models_expanded,
                cx.listener(|this, _, _, cx| {
                    this.models_expanded = !this.models_expanded;
                    this.serialize(cx);
                    cx.notify();
                }),
                cx,
            ))
            .when(models_expanded, |this| {
                if let Some(summary) = &self.data.summary {
                    let mut children = v_flex().w_full().px_3().pb_1().gap_0p5();
                    for group in summary.groups.iter().take(5) {
                        children = children.child(
                            h_flex()
                                .w_full()
                                .gap_1()
                                .child(
                                    div()
                                        .w(px(6.))
                                        .h(px(6.))
                                        .rounded_full()
                                        .flex_none()
                                        .bg(Color::Accent.color(cx)),
                                )
                                .child(
                                    Label::new(shorten_model_name(&group.key))
                                        .size(LabelSize::Small)
                                        .truncate(),
                                )
                                .child(
                                    Label::new(format!("${:.2}", group.total_cost_usd))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        );
                    }
                    this.child(children)
                } else {
                    this.child(
                        div().px_3().pb_1().child(
                            Label::new("No data")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                }
            })
    }

    fn render_waste_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let waste_expanded = self.waste_expanded;
        v_flex()
            .w_full()
            .child(Self::render_section_header(
                "section-waste",
                "Waste Score",
                waste_expanded,
                cx.listener(|this, _, _, cx| {
                    this.waste_expanded = !this.waste_expanded;
                    this.serialize(cx);
                    cx.notify();
                }),
                cx,
            ))
            .when(waste_expanded, |this| {
                if let Some(waste) = &self.data.waste_score {
                    this.child(
                        v_flex()
                            .w_full()
                            .px_3()
                            .pb_1()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .child(
                                        Label::new("Waste %")
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        Label::new(format!("{:.1}%", waste.waste_score * 100.0))
                                            .size(LabelSize::Small),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .child(
                                        Label::new("Est. Waste")
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        Label::new(format!(
                                            "${:.2}/{}",
                                            waste.estimated_waste_usd, waste.total_cost_usd
                                        ))
                                        .size(LabelSize::Small),
                                    ),
                            ),
                    )
                } else {
                    this.child(
                        div().px_3().pb_1().child(
                            Label::new("No data")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                }
            })
    }

    fn render_routing_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let routing_expanded = self.routing_expanded;
        let section_label = if let Some(p) = &self.data.policy {
            format!("Routing Policy (v{})", p.version)
        } else {
            "Routing Policy".into()
        };
        v_flex()
            .w_full()
            .child(Self::render_section_header(
                "section-routing",
                &section_label,
                routing_expanded,
                cx.listener(|this, _, _, cx| {
                    this.routing_expanded = !this.routing_expanded;
                    this.serialize(cx);
                    cx.notify();
                }),
                cx,
            ))
            .when(routing_expanded, |this| {
                if let Some(policy) = &self.data.policy {
                    let mut children = v_flex().w_full().px_3().pb_1().gap_0p5();
                    for rule in policy.rules.iter().take(5) {
                        let criteria_str = format!("{:?}", rule.criteria)
                            .to_lowercase()
                            .replace('_', " ");
                        children = children.child(
                            h_flex()
                                .w_full()
                                .gap_1()
                                .child(
                                    Label::new(rule.task_type.clone())
                                        .size(LabelSize::Small)
                                        .truncate(),
                                )
                                .child(
                                    Label::new(format!(
                                        "→ {} >{:.2}",
                                        criteria_str, rule.min_quality
                                    ))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                                ),
                        );
                    }
                    this.child(children)
                } else {
                    this.child(
                        div().px_3().pb_1().child(
                            Label::new("No data")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                }
            })
    }

    fn render_agents_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let agents_expanded = self.agents_expanded;
        v_flex()
            .w_full()
            .child(Self::render_section_header(
                "section-agents",
                "Agent Performance",
                agents_expanded,
                cx.listener(|this, _, _, cx| {
                    this.agents_expanded = !this.agents_expanded;
                    this.serialize(cx);
                    cx.notify();
                }),
                cx,
            ))
            .when(agents_expanded, |this| {
                if let Some(metrics) = &self.data.agent_metrics {
                    if metrics.agents.is_empty() {
                        return this.child(
                            div().px_3().pb_1().child(
                                Label::new("No agent data yet")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                        );
                    }
                    let mut children = v_flex().w_full().px_3().pb_1().gap_0p5();
                    for agent in metrics.agents.iter().take(8) {
                        let failure_rate = if agent.request_count > 0 {
                            agent.failure_count as f64 / agent.request_count as f64 * 100.0
                        } else {
                            0.0
                        };
                        children = children.child(
                            v_flex()
                                .w_full()
                                .gap_0p5()
                                .py_0p5()
                                .border_b_1()
                                .border_color(cx.theme().colors().border_variant)
                                .child(
                                    h_flex()
                                        .w_full()
                                        .justify_between()
                                        .child(
                                            Label::new(agent.agent_name.clone())
                                                .size(LabelSize::Small)
                                                .truncate(),
                                        )
                                        .child(
                                            Label::new(format!("${:.3}", agent.total_cost_usd))
                                                .size(LabelSize::Small),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_2()
                                        .child(
                                            Label::new(format!(
                                                "{}req",
                                                format_number(agent.request_count)
                                            ))
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                        )
                                        .child(
                                            Label::new(format!(
                                                "{}tok",
                                                format_tokens(agent.total_tokens)
                                            ))
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                        )
                                        .child(
                                            Label::new(format!("{:.0}ms", agent.avg_latency_ms))
                                                .size(LabelSize::Small)
                                                .color(Color::Muted),
                                        )
                                        .when(failure_rate > 0.0, |this| {
                                            this.child(
                                                Label::new(format!("{:.1}%err", failure_rate))
                                                    .size(LabelSize::Small)
                                                    .color(Color::Error),
                                            )
                                        }),
                                ),
                        );
                    }
                    this.child(children)
                } else {
                    this.child(
                        div().px_3().pb_1().child(
                            Label::new("No data")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                }
            })
    }
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn shorten_model_name(name: &str) -> String {
    let parts: Vec<&str> = name.split('/').collect();
    if parts.len() > 1 {
        parts[parts.len() - 1].to_string()
    } else {
        name.to_string()
    }
}

impl EventEmitter<Event> for PrismDashboardPanel {}
impl EventEmitter<PanelEvent> for PrismDashboardPanel {}

impl Focusable for PrismDashboardPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PrismDashboardPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let gateway_label = self
            .gateway_url
            .clone()
            .unwrap_or_else(|| "no gateway configured".into());

        v_flex()
            .key_context("PrismDashboard")
            .track_focus(&self.focus_handle)
            .size_full()
            // Header
            .child(
                h_flex()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .h(px(32.))
                    .flex_none()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(Label::new("PrisM Dashboard").size(LabelSize::Small))
                    .child(
                        IconButton::new("refresh", IconName::ArrowCircle)
                            .icon_size(ui::IconSize::Small)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh(cx);
                            })),
                    ),
            )
            // Body
            .child(
                v_flex()
                    .id("prism-dashboard-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .when_some(self.error.clone(), |this, err| {
                        this.child(
                            div()
                                .px_2()
                                .py_1()
                                .child(Label::new(err).size(LabelSize::Small).color(Color::Error)),
                        )
                    })
                    .when(self.is_loading && self.data.summary.is_none(), |this| {
                        this.child(
                            div().px_2().py_1().child(
                                Label::new("Loading…")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                        )
                    })
                    .child(self.render_cost_section(cx))
                    .child(self.render_models_section(cx))
                    .child(self.render_waste_section(cx))
                    .child(self.render_routing_section(cx))
                    .child(self.render_agents_section(cx))
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .border_t_1()
                            .border_color(cx.theme().colors().border)
                            .child(
                                Label::new(gateway_label)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    ),
            )
    }
}

impl Panel for PrismDashboardPanel {
    fn persistent_name() -> &'static str {
        "PrismDashboardPanel"
    }

    fn panel_key() -> &'static str {
        PANEL_KEY
    }

    fn position(&self, _: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, _position: DockPosition, _: &mut Window, _cx: &mut Context<Self>) {}

    fn size(&self, _: &Window, _cx: &App) -> Pixels {
        self.width.unwrap_or(px(320.))
    }

    fn set_size(&mut self, size: Option<Pixels>, _: &mut Window, cx: &mut Context<Self>) {
        self.width = size;
        self.serialize(cx);
        cx.notify();
    }

    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.active = active;
        cx.notify();
    }

    fn icon(&self, _: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Sparkle)
    }

    fn icon_tooltip(&self, _: &Window, _cx: &App) -> Option<&'static str> {
        Some("PrisM Dashboard")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        8
    }
}
