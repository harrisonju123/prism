use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use futures::AsyncReadExt as _;
use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, Render, Task, WeakEntity,
    Window, actions, px,
};
use http_client::{AsyncBody, HttpClientWithUrl, Method};
use project::project_settings::ProjectSettings;
use settings::Settings as _;
use ui::{
    Button, ButtonStyle, Color, Divider, DividerColor, Icon, IconName, Label, LabelSize, Tooltip,
    h_flex, prelude::*, v_flex,
};
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(prism_hq, [TogglePostmanPanel]);

const POSTMAN_PANEL_KEY: &str = "postman_panel";

// ── Lightweight Postman API client ────────────────────────────────────────

struct PostmanHttpClient {
    http_client: Arc<HttpClientWithUrl>,
    api_key: String,
}

impl PostmanHttpClient {
    fn new(http_client: Arc<HttpClientWithUrl>, api_key: String) -> Self {
        Self {
            http_client,
            api_key,
        }
    }

    async fn get_json(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("https://api.postman.com{path}");
        let request = http_client::Request::builder()
            .method(Method::GET)
            .uri(&url)
            .header("X-Api-Key", &self.api_key)
            .header("Accept", "application/json")
            .body(AsyncBody::default())?;

        let mut response = self
            .http_client
            .send(request)
            .await
            .with_context(|| format!("request to {url} failed"))?;

        let mut body = Vec::new();
        response
            .body_mut()
            .read_to_end(&mut body)
            .await
            .context("error reading response body")?;

        if !response.status().is_success() {
            let text = String::from_utf8_lossy(&body);
            bail!(
                "Postman API returned {}: {text}",
                response.status().as_u16()
            );
        }

        serde_json::from_slice(&body).context("failed to parse Postman API response")
    }

    async fn execute_http(
        &self,
        method: &str,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<String> {
        let start = std::time::Instant::now();

        let mut builder = http_client::Request::builder()
            .method(method.parse::<Method>().unwrap_or(Method::GET))
            .uri(url);

        for (k, v) in headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        let request = builder.body(AsyncBody::default())?;

        let mut response = self
            .http_client
            .send(request)
            .await
            .with_context(|| format!("request to {url} failed"))?;
        let elapsed_ms = start.elapsed().as_millis();

        let status = response.status().as_u16();
        let mut resp_body = Vec::new();
        response
            .body_mut()
            .read_to_end(&mut resp_body)
            .await
            .context("error reading response body")?;
        let body_str = String::from_utf8_lossy(&resp_body).into_owned();

        let body_display = serde_json::from_str::<serde_json::Value>(&body_str)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or(body_str);

        Ok(format!(
            "Status: {status}\nDuration: {elapsed_ms}ms\n\n{body_display}"
        ))
    }
}

// ── Domain types used by the panel ────────────────────────────────────────

#[derive(Debug, Clone)]
struct CollectionSummary {
    id: String,
    name: String,
}

#[derive(Debug, Clone)]
struct EnvironmentSummary {
    id: String,
    name: String,
}

#[derive(Debug, Clone)]
struct RequestItem {
    name: String,
    method: String,
    url: String,
    /// Full path within the collection, e.g. "Folder/Request Name".
    path: String,
    headers: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct CollectionDetail {
    requests: Vec<RequestItem>,
}

#[derive(Debug, Clone)]
struct ResponseView {
    text: String,
}

// ── Panel ─────────────────────────────────────────────────────────────────

pub struct PostmanPanel {
    focus_handle: FocusHandle,
    position: DockPosition,
    width: Option<gpui::Pixels>,
    http_client: Arc<HttpClientWithUrl>,

    collections: Vec<CollectionSummary>,
    environments: Vec<EnvironmentSummary>,
    selected_collection: Option<String>,
    collection_detail: Option<CollectionDetail>,
    selected_request: Option<usize>,
    last_response: Option<ResponseView>,
    is_loading: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
}

impl EventEmitter<PanelEvent> for PostmanPanel {}

impl PostmanPanel {
    pub fn new(
        http_client: Arc<HttpClientWithUrl>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let mut panel = Self {
            focus_handle,
            position: DockPosition::Right,
            width: None,
            http_client,
            collections: Vec::new(),
            environments: Vec::new(),
            selected_collection: None,
            collection_detail: None,
            selected_request: None,
            last_response: None,
            is_loading: false,
            error: None,
            refresh_task: None,
        };
        panel.refresh(cx);
        panel
    }

    fn api_client(&self, cx: &App) -> Option<Arc<PostmanHttpClient>> {
        let settings = ProjectSettings::get_global(cx).postman.clone();
        if !settings.enabled {
            return None;
        }
        let api_key = settings.api_key?;
        Some(Arc::new(PostmanHttpClient::new(
            self.http_client.clone(),
            api_key,
        )))
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.api_client(cx) else {
            self.error = Some("Postman not configured. Add `\"postman\": { \"api_key\": \"...\", \"enabled\": true }` to .zed/settings.json".to_string());
            cx.notify();
            return;
        };

        self.is_loading = true;
        self.error = None;
        cx.notify();

        self.refresh_task = Some(cx.spawn(async move |this: WeakEntity<PostmanPanel>, cx| {
            let result: Result<(Vec<CollectionSummary>, Vec<EnvironmentSummary>)> = cx
                .background_spawn(async move {
                    let (cols_json, envs_json) = futures::join!(
                        client.get_json("/collections"),
                        client.get_json("/environments"),
                    );
                    let collections = parse_collections(cols_json?);
                    let environments = parse_environments(envs_json?);
                    anyhow::Ok((collections, environments))
                })
                .await;

            this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok((collections, environments)) => {
                        this.collections = collections;
                        this.environments = environments;
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

    fn load_collection(&mut self, collection_id: String, cx: &mut Context<Self>) {
        let Some(client) = self.api_client(cx) else {
            return;
        };
        self.selected_collection = Some(collection_id.clone());
        self.collection_detail = None;
        self.selected_request = None;
        cx.notify();

        cx.spawn(async move |this: WeakEntity<PostmanPanel>, cx| {
            let result: Result<CollectionDetail> = cx
                .background_spawn(async move {
                    let json = client.get_json(&format!("/collections/{collection_id}")).await?;
                    anyhow::Ok(parse_collection_detail(&json))
                })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok(detail) => this.collection_detail = Some(detail),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run_selected_request(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.api_client(cx) else {
            return;
        };
        let Some(detail) = &self.collection_detail else {
            return;
        };
        let Some(idx) = self.selected_request else {
            return;
        };
        let Some(req) = detail.requests.get(idx) else {
            return;
        };

        let method = req.method.clone();
        let url = req.url.clone();
        let headers = req.headers.clone();
        self.last_response = None;
        cx.notify();

        cx.spawn(async move |this: WeakEntity<PostmanPanel>, cx| {
            let result: Result<String> = cx
                .background_spawn(async move {
                    client.execute_http(&method, &url, &headers).await
                })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok(text) => this.last_response = Some(ResponseView { text }),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn method_color(method: &str) -> Color {
    match method {
        "GET" => Color::Success,
        "POST" => Color::Accent,
        "PUT" | "PATCH" => Color::Warning,
        "DELETE" => Color::Error,
        _ => Color::Default,
    }
}

/// Parse a top-level JSON array keyed by `key`, mapping each element with `f`.
fn parse_resource_list<T>(
    json: &serde_json::Value,
    key: &str,
    f: impl Fn(&serde_json::Value) -> Option<T>,
) -> Vec<T> {
    json[key]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(f)
        .collect()
}

fn parse_collections(json: serde_json::Value) -> Vec<CollectionSummary> {
    parse_resource_list(&json, "collections", |c| {
        Some(CollectionSummary {
            id: c["id"].as_str().or_else(|| c["uid"].as_str())?.to_string(),
            name: c["name"].as_str()?.to_string(),
        })
    })
}

fn parse_environments(json: serde_json::Value) -> Vec<EnvironmentSummary> {
    parse_resource_list(&json, "environments", |e| {
        Some(EnvironmentSummary {
            id: e["id"].as_str().or_else(|| e["uid"].as_str())?.to_string(),
            name: e["name"].as_str()?.to_string(),
        })
    })
}

fn parse_collection_detail(json: &serde_json::Value) -> CollectionDetail {
    let mut requests = Vec::new();
    let items = json["collection"]["item"]
        .as_array()
        .or_else(|| json["item"].as_array());

    if let Some(items) = items {
        collect_requests(items, "", &mut requests);
    }

    CollectionDetail { requests }
}

fn collect_requests(items: &[serde_json::Value], prefix: &str, out: &mut Vec<RequestItem>) {
    for item in items {
        let name = item["name"].as_str().unwrap_or("").to_string();
        let full_path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };

        if item["request"].is_object() {
            let method = item["request"]["method"]
                .as_str()
                .unwrap_or("GET")
                .to_uppercase();
            let url = item["request"]["url"]
                .as_str()
                .or_else(|| item["request"]["url"]["raw"].as_str())
                .unwrap_or("")
                .to_string();
            let headers = item["request"]["header"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter_map(|h| {
                    let key = h["key"].as_str()?;
                    let val = h["value"].as_str().unwrap_or("");
                    if key.is_empty() {
                        return None;
                    }
                    Some((key.to_string(), val.to_string()))
                })
                .collect();
            out.push(RequestItem {
                name,
                method,
                url,
                path: full_path,
                headers,
            });
        } else if let Some(children) = item["item"].as_array() {
            collect_requests(children, &full_path, out);
        }
    }
}

impl Focusable for PostmanPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PostmanPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_loading = self.is_loading;

        let mut content = v_flex().flex_1().overflow_hidden().p_2().gap_2();

        if is_loading {
            content = content.child(
                Label::new("Loading…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        }

        if let Some(err) = &self.error {
            content = content.child(
                Label::new(format!("Error: {err}"))
                    .size(LabelSize::Small)
                    .color(Color::Error),
            );
        }

        // ── Collections list ──────────────────────────────────────────────
        if !self.collections.is_empty() {
            content = content.child(
                Label::new("Collections")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );

            for col in &self.collections {
                let col_id = col.id.clone();
                let is_selected = self.selected_collection.as_deref() == Some(col_id.as_str());
                let row = h_flex()
                    .id(gpui::SharedString::from(format!("col_{col_id}")))
                    .gap_1()
                    .cursor_pointer()
                    .px_1()
                    .rounded_md()
                    .when(is_selected, |this| {
                        this.bg(cx.theme().colors().element_selected)
                    })
                    .child(
                        Icon::new(IconName::Folder)
                            .size(ui::IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(col.name.chars().take(40).collect::<String>())
                            .size(LabelSize::Small),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_collection(col_id.clone(), cx);
                    }));
                content = content.child(row);
            }
        } else if !is_loading && self.error.is_none() {
            content = content.child(
                Label::new("No collections")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        }

        // ── Request list for selected collection ──────────────────────────
        if let Some(detail) = &self.collection_detail {
            content = content.child(Divider::horizontal().color(DividerColor::Border));
            content = content.child(
                Label::new("Requests")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );

            for (idx, req) in detail.requests.iter().enumerate() {
                let is_selected = self.selected_request == Some(idx);
                let method_color = method_color(&req.method);
                let row = h_flex()
                    .id(gpui::SharedString::from(format!("req_{idx}")))
                    .gap_1()
                    .cursor_pointer()
                    .px_1()
                    .rounded_md()
                    .when(is_selected, |this| {
                        this.bg(cx.theme().colors().element_selected)
                    })
                    .child(
                        Label::new(req.method.chars().take(6).collect::<String>())
                            .size(LabelSize::Small)
                            .color(method_color),
                    )
                    .child(
                        Label::new(req.name.chars().take(35).collect::<String>())
                            .size(LabelSize::Small),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_request = Some(idx);
                        cx.notify();
                    }));
                content = content.child(row);
            }

            // Run button for selected request
            if self.selected_request.is_some() {
                content = content.child(
                    Button::new("run-request", "▶ Run")
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_selected_request(cx);
                        })),
                );
            }
        }

        // ── Response viewer ───────────────────────────────────────────────
        if let Some(response) = &self.last_response {
            content = content.child(Divider::horizontal().color(DividerColor::Border));
            content = content.child(
                Label::new("Response")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
            content = content.child(
                Label::new(response.text.chars().take(500).collect::<String>())
                    .size(LabelSize::Small),
            );
        }

        v_flex()
            .size_full()
            .overflow_hidden()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Icon::new(IconName::Link)
                            .size(ui::IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new("Postman")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(gpui::div().flex_1())
                    .child(
                        Button::new("refresh-postman", "↻")
                            .style(ButtonStyle::Transparent)
                            .tooltip(Tooltip::text("Refresh collections"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh(cx);
                            })),
                    ),
            )
            .child(content)
    }
}

impl Panel for PostmanPanel {
    fn persistent_name() -> &'static str {
        "PostmanPanel"
    }

    fn panel_key() -> &'static str {
        POSTMAN_PANEL_KEY
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.position = position;
        cx.notify();
    }

    fn size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        self.width.unwrap_or(px(350.0))
    }

    fn set_size(
        &mut self,
        size: Option<gpui::Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.width = size;
        cx.notify();
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<ui::IconName> {
        Some(ui::IconName::Link)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Postman")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(TogglePostmanPanel)
    }

    fn activation_priority(&self) -> u32 {
        8
    }
}
