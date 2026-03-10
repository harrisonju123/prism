use crate::service::get_uglyhat_handle;
use crate::types::SessionEntry;
use anyhow::Result;
use db::kvp::KEY_VALUE_STORE;
use futures::AsyncReadExt as _;
use gpui::{
    actions, px, Action, App, AsyncWindowContext, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, IntoElement, ParentElement, Pixels, Render, Styled, Task, WeakEntity,
    Window,
};
use http_client::{AsyncBody, HttpClient};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use ui::{h_flex, prelude::*, v_flex, Color, IconButton, IconName, Label, LabelSize, Tooltip};
use util::{ResultExt, TryFutureExt};
use workspace::{
    dock::{DockPosition, Panel, PanelEvent},
    Workspace,
};

const PANEL_KEY: &str = "SessionHistoryPanel";

fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    result
}
const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(120);

actions!(
    session_history,
    [
        /// Toggles the session history panel.
        Toggle,
        /// Toggles focus on the session history panel.
        ToggleFocus
    ]
);

#[derive(Default)]
enum ViewState {
    #[default]
    List,
    Detail(SessionEntry),
}

pub struct SessionHistoryPanel {
    focus_handle: FocusHandle,
    width: Option<Pixels>,
    active: bool,
    sessions: Vec<SessionEntry>,
    is_loading: bool,
    error: Option<String>,
    search_query: String,
    view_state: ViewState,
    refresh_task: Option<Task<()>>,
    _auto_refresh: Task<()>,
    pending_serialization: Task<Option<()>>,
    thread_cost: Option<prism_types::ThreadCostResponse>,
    cost_task: Option<Task<()>>,
    prism_api_url: Option<String>,
    prism_api_key: Option<String>,
    http_client: Arc<dyn HttpClient>,
}

#[derive(Serialize, Deserialize)]
struct SerializedPanel {
    width: Option<Pixels>,
}

#[derive(Debug)]
pub enum Event {
    DockPositionChanged,
    Focus,
    Dismissed,
}

impl SessionHistoryPanel {
    pub fn new(
        _workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let http_client = cx.http_client();
        cx.new(|cx| {
            let mut panel = Self {
                focus_handle: cx.focus_handle(),
                width: None,
                active: false,
                sessions: Vec::new(),
                is_loading: false,
                error: None,
                search_query: String::new(),
                view_state: ViewState::List,
                refresh_task: None,
                _auto_refresh: Task::ready(()),
                pending_serialization: Task::ready(None),
                thread_cost: None,
                cost_task: None,
                prism_api_url: std::env::var("PRISM_API_URL")
                    .ok()
                    .or_else(|| Some("http://localhost:3000".to_string())),
                prism_api_key: std::env::var("PRISM_API_KEY").ok(),
                http_client,
            };

            let auto_refresh = cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(AUTO_REFRESH_INTERVAL).await;
                this.update(cx, |panel: &mut SessionHistoryPanel, cx| {
                    if panel.active {
                        panel.refresh(cx);
                    }
                })
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
                        cx.notify();
                    });
                }
                panel
            })
        })
    }

    fn serialize(&mut self, cx: &mut Context<Self>) {
        let width = self.width;
        self.pending_serialization = cx.background_spawn(
            async move {
                KEY_VALUE_STORE
                    .write_kvp(
                        PANEL_KEY.into(),
                        serde_json::to_string(&SerializedPanel { width })?,
                    )
                    .await?;
                anyhow::Ok(())
            }
            .log_err(),
        );
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.is_loading = true;
        self.error = None;
        cx.notify();

        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let handle = get_uglyhat_handle(&this, cx);

            let result: anyhow::Result<Vec<SessionEntry>> = cx
                .background_spawn(async move {
                    let Some(handle) = handle else {
                        anyhow::bail!("uglyhat service not available");
                    };
                    let activities = handle.list_activity(uglyhat::store::ActivityFilters {
                        limit: 50,
                        ..Default::default()
                    })?;

                    let sessions: Vec<SessionEntry> = activities
                        .into_iter()
                        .enumerate()
                        .map(|(i, a)| {
                            let date = a.created_at.format("%Y-%m-%d").to_string();
                            let entity_name = a.summary.clone();
                            let summary_text = format!("{} {}", a.action, &entity_name);
                            SessionEntry {
                                id: format!("activity-{i}"),
                                agent_name: a.actor.clone(),
                                date,
                                task_name: Some(entity_name),
                                task_id: None,
                                thread_id: Some(a.entity_id.to_string()),
                                action: a.action.clone(),
                                summary: summary_text,
                            }
                        })
                        .collect();
                    anyhow::Ok(sessions)
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok(sessions) => {
                        this.sessions = sessions;
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

    fn filtered_sessions(&self) -> Vec<SessionEntry> {
        if self.search_query.is_empty() {
            return self.sessions.clone();
        }
        let q = self.search_query.to_lowercase();
        self.sessions
            .iter()
            .filter(|s| {
                s.agent_name.to_lowercase().contains(&q)
                    || s.summary.to_lowercase().contains(&q)
                    || s.task_name
                        .as_deref()
                        .map(|t| t.to_lowercase().contains(&q))
                        .unwrap_or(false)
                    || s.date.contains(&q)
            })
            .cloned()
            .collect()
    }

    fn open_detail(&mut self, session: SessionEntry, cx: &mut Context<Self>) {
        self.thread_cost = None;
        self.view_state = ViewState::Detail(session.clone());
        cx.notify();
        self.fetch_thread_cost(&session, cx);
    }

    fn fetch_thread_cost(&mut self, session: &SessionEntry, cx: &mut Context<Self>) {
        let Some(api_url) = self.prism_api_url.clone() else {
            return;
        };
        let Some(api_key) = self.prism_api_key.clone() else {
            return;
        };
        let Some(thread_id) = session.thread_id.clone() else {
            return;
        };

        let http_client = self.http_client.clone();
        self.cost_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let url = format!(
                        "{api_url}/v1/costs?thread_id={thread_id}",
                        thread_id = urlencoding(&thread_id),
                    );
                    let request = http_client::http::Request::builder()
                        .uri(&url)
                        .header("Authorization", format!("Bearer {api_key}"))
                        .body(AsyncBody::empty())?;
                    let mut response = http_client.send(request).await?;
                    let mut body = Vec::new();
                    response.body_mut().read_to_end(&mut body).await?;
                    if !response.status().is_success() {
                        anyhow::bail!("cost fetch failed: {}", response.status());
                    }
                    let cost = serde_json::from_slice::<prism_types::ThreadCostResponse>(&body)?;
                    anyhow::Ok(cost)
                })
                .await;
            this.update(cx, |this, cx| {
                this.cost_task = None;
                if let Ok(cost) = result {
                    this.thread_cost = Some(cost);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.view_state = ViewState::List;
        self.thread_cost = None;
        self.cost_task = None;
        cx.notify();
    }

    fn render_session_row(session: &SessionEntry, cx: &App) -> gpui::Div {
        let dot_color = if session.action == "completed" {
            Color::Success
        } else if session.action == "created" {
            Color::Accent
        } else {
            Color::Muted
        };

        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .hover(|style| style.bg(cx.theme().colors().element_hover))
            .child(
                div()
                    .w(px(6.))
                    .h(px(6.))
                    .rounded_full()
                    .flex_none()
                    .bg(dot_color.color(cx)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Label::new(session.agent_name.clone())
                            .size(LabelSize::Small)
                            .truncate(),
                    )
                    .child(
                        Label::new(session.summary.clone())
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    ),
            )
            .child(
                Label::new(session.date.clone())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
    }

    fn render_detail_view(&self, session: &SessionEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let session = session.clone();
        let cost = self.thread_cost.clone();

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .h(px(32.))
                    .flex_none()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        ui::IconButton::new("back", IconName::ArrowLeft)
                            .icon_size(ui::IconSize::Small)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Back to list"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_detail(cx))),
                    )
                    .child(
                        Label::new(session.agent_name.clone())
                            .size(LabelSize::Small)
                            .truncate(),
                    ),
            )
            .child(
                v_flex()
                    .id("session-detail-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Label::new("Date:")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(Label::new(session.date.clone()).size(LabelSize::Small)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Label::new("Action:")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(Label::new(session.action.clone()).size(LabelSize::Small)),
                    )
                    .when_some(session.task_name.clone(), |this, t| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Label::new("Task:")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(Label::new(t).size(LabelSize::Small)),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                Label::new("Summary")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(Label::new(session.summary).size(LabelSize::Small)),
                    )
                    .when_some(cost, |this, cost| {
                        this.child(
                            v_flex()
                                .gap_0p5()
                                .mt_2()
                                .pt_2()
                                .border_t_1()
                                .border_color(cx.theme().colors().border)
                                .child(
                                    Label::new("Cost")
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(Label::new(format!("${:.4}", cost.total_cost_usd)).size(LabelSize::Small))
                                        .child(Label::new(format!("{} requests", cost.request_count)).size(LabelSize::Small).color(Color::Muted)),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(Label::new(format!("{} in", cost.total_input_tokens)).size(LabelSize::Small).color(Color::Muted))
                                        .child(Label::new(format!("{} out", cost.total_output_tokens)).size(LabelSize::Small).color(Color::Muted)),
                                ),
                        )
                    }),
            )
    }
}

impl EventEmitter<Event> for SessionHistoryPanel {}
impl EventEmitter<PanelEvent> for SessionHistoryPanel {}

impl Focusable for SessionHistoryPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SessionHistoryPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let ViewState::Detail(ref session) = self.view_state {
            let session = session.clone();
            return v_flex()
                .key_context("SessionHistory")
                .track_focus(&self.focus_handle)
                .size_full()
                .child(self.render_detail_view(&session, cx))
                .into_any_element();
        }

        let sessions = self.filtered_sessions();

        v_flex()
            .key_context("SessionHistory")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                h_flex()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .h(px(32.))
                    .flex_none()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(Label::new("Session History").size(LabelSize::Small))
                    .child(
                        IconButton::new("refresh", IconName::ArrowCircle)
                            .icon_size(ui::IconSize::Small)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                v_flex()
                    .id("session-history-body")
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
                    .when(self.is_loading && sessions.is_empty(), |this| {
                        this.child(
                            div().px_2().py_1().child(
                                Label::new("Loading…")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                        )
                    })
                    .when(sessions.is_empty() && !self.is_loading, |this| {
                        this.child(
                            div().px_2().py_1().child(
                                Label::new("No session history found.")
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                        )
                    })
                    .children(sessions.iter().map(|session| {
                        let session_clone = session.clone();
                        Self::render_session_row(session, cx)
                            .id(ElementId::Name(session.id.clone().into()))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_detail(session_clone.clone(), cx);
                            }))
                    })),
            )
            .into_any_element()
    }
}

impl Panel for SessionHistoryPanel {
    fn persistent_name() -> &'static str {
        "SessionHistoryPanel"
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
        self.width.unwrap_or(px(300.))
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
        Some(IconName::HistoryRerun)
    }

    fn icon_tooltip(&self, _: &Window, _cx: &App) -> Option<&'static str> {
        Some("Session History")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        10
    }
}
