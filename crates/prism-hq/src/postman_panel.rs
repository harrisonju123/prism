use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use futures::AsyncReadExt as _;
use fs::Fs;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, Task,
    WeakEntity, Window, actions, px,
};
use http_client::{AsyncBody, HttpClientWithUrl, Method};
use project::project_settings::ProjectSettings;
use settings::{Settings as _, update_settings_file};
use ui::{
    Button, ButtonStyle, Color, ContextMenu, ContextMenuEntry, Divider, DividerColor, Icon,
    IconName, Label, LabelSize, PopoverMenu, PopoverMenuHandle, Tooltip,
    h_flex, prelude::*, v_flex,
};
use ui_input::InputField;
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

    async fn execute_request(
        &self,
        request_def: &serde_json::Value,
        env_vars: &HashMap<String, String>,
    ) -> Result<ExecutionResult> {
        let method_str = request_def["method"]
            .as_str()
            .unwrap_or("GET")
            .to_uppercase();
        let url_raw = extract_url(request_def).context("could not determine request URL")?;
        let url = substitute_vars(&url_raw, env_vars);

        let method = method_str
            .parse::<Method>()
            .with_context(|| format!("unknown HTTP method: {method_str}"))?;

        let mut builder = http_client::Request::builder().method(method).uri(&url);

        if let Some(headers) = request_def["header"].as_array() {
            for h in headers {
                let key = h["key"].as_str().unwrap_or("");
                let val = h["value"].as_str().unwrap_or("");
                if !key.is_empty() {
                    builder = builder.header(key, substitute_vars(val, env_vars));
                }
            }
        }

        let body = build_body(request_def, env_vars);
        let request = builder.body(body)?;

        let start = std::time::Instant::now();
        let mut response = self
            .http_client
            .send(request)
            .await
            .with_context(|| format!("request to {url} failed"))?;
        let duration_ms = start.elapsed().as_millis();

        let status = response.status().as_u16();
        let mut headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
            .collect();
        headers.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        let mut resp_body_bytes = Vec::new();
        response
            .body_mut()
            .read_to_end(&mut resp_body_bytes)
            .await
            .context("error reading response body")?;
        let body_text = String::from_utf8(resp_body_bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());

        // Pretty-print JSON if possible; intern as SharedString so render clones are O(1).
        let body: gpui::SharedString = serde_json::from_str::<serde_json::Value>(&body_text)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or(body_text)
            .into();

        Ok(ExecutionResult {
            status,
            duration_ms,
            headers,
            body,
        })
    }
}

// ── Pure helpers (ported from ide/agent/src/tools/postman.rs) ─────────────

/// Extract a URL string from the Postman request object's `url` field, which may
/// be a plain string, an object with a `raw` field, or a fully decomposed object.
fn extract_url(request_def: &serde_json::Value) -> Option<String> {
    let url = &request_def["url"];
    if let Some(s) = url.as_str().or_else(|| url["raw"].as_str()) {
        return Some(s.to_string());
    }
    // Reconstruct from decomposed parts when `raw` is absent (rare in practice).
    let protocol = url["protocol"].as_str().unwrap_or("https");
    let host = url["host"]
        .as_array()?
        .iter()
        .filter_map(|h| h.as_str())
        .collect::<Vec<_>>()
        .join(".");
    if host.is_empty() {
        return None;
    }
    let path = url["path"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default();
    let query: Option<String> = url["query"].as_array().and_then(|params| {
        let qs = params
            .iter()
            .filter_map(|p| Some(format!("{}={}", p["key"].as_str()?, p["value"].as_str()?)))
            .collect::<Vec<_>>()
            .join("&");
        if qs.is_empty() { None } else { Some(qs) }
    });
    let mut result = format!("{protocol}://{host}");
    if !path.is_empty() {
        result.push('/');
        result.push_str(&path);
    }
    if let Some(qs) = query {
        result.push('?');
        result.push_str(&qs);
    }
    Some(result)
}

fn substitute_vars(s: &str, vars: &HashMap<String, String>) -> String {
    // Short-circuit: most strings have no placeholders.
    if !s.contains("{{") {
        return s.to_string();
    }
    let mut result = s.to_string();
    for (k, v) in vars {
        result = result.replace(&format!("{{{{{k}}}}}"), v);
    }
    result
}

fn build_body(request_def: &serde_json::Value, vars: &HashMap<String, String>) -> AsyncBody {
    let body = &request_def["body"];
    let mode = body["mode"].as_str().unwrap_or("");
    match mode {
        "raw" => {
            let raw = body["raw"].as_str().unwrap_or("");
            AsyncBody::from(substitute_vars(raw, vars))
        }
        "urlencoded" => {
            if let Some(items) = body["urlencoded"].as_array() {
                let encoded = items
                    .iter()
                    .filter_map(|item| {
                        let k = item["key"].as_str()?;
                        let v = item["value"].as_str().unwrap_or("");
                        Some(format!(
                            "{}={}",
                            urlencoding::encode(k),
                            urlencoding::encode(&substitute_vars(v, vars))
                        ))
                    })
                    .fold(String::new(), |mut acc, pair| {
                        if !acc.is_empty() {
                            acc.push('&');
                        }
                        acc.push_str(&pair);
                        acc
                    });
                AsyncBody::from(encoded)
            } else {
                AsyncBody::default()
            }
        }
        _ => AsyncBody::default(),
    }
}

fn parse_env_vars(env_json: &serde_json::Value) -> HashMap<String, String> {
    env_json["environment"]["values"]
        .as_array()
        .or_else(|| env_json["values"].as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter(|v| v["enabled"].as_bool().unwrap_or(true))
        .filter_map(|v| {
            let k = v["key"].as_str()?;
            let val = v["value"].as_str()?;
            Some((k.to_string(), val.to_string()))
        })
        .collect()
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
    /// Full path within the collection, e.g. "Folder/Request Name".
    path: String,
    /// Arc-wrapped so cloning a RequestItem (or the entire CollectionDetail) is cheap,
    /// and the background-task move in run_selected_request doesn't deep-copy the JSON.
    request_def: Arc<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct CollectionDetail {
    requests: Vec<RequestItem>,
}

#[derive(Debug, Clone)]
struct ExecutionResult {
    status: u16,
    duration_ms: u128,
    /// Response headers sorted by name — sorted once at creation, never re-sorted in render.
    headers: Vec<(String, String)>,
    /// Arc-backed so render clones are cheap regardless of response size.
    body: gpui::SharedString,
}

#[derive(Debug, Clone)]
enum PanelView {
    CollectionsList,
    RequestDetail { request_idx: usize },
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
    last_response: Option<ExecutionResult>,
    is_loading: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
    api_key_input: Entity<InputField>,

    view: PanelView,
    selected_environment: Option<String>,
    env_vars: Arc<HashMap<String, String>>,
    show_request_headers: bool,
    show_request_body: bool,
    show_response_headers: bool,
    env_selector_handle: PopoverMenuHandle<ContextMenu>,
    env_load_task: Option<Task<()>>,
    collection_load_task: Option<Task<()>>,
    request_task: Option<Task<()>>,
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

        let api_key_input = cx.new(|cx| InputField::new(window, cx, "PMAK-xxxxxxxx…"));

        let mut panel = Self {
            focus_handle,
            position: DockPosition::Right,
            width: None,
            http_client,
            collections: Vec::new(),
            environments: Vec::new(),
            selected_collection: None,
            collection_detail: None,
            last_response: None,
            is_loading: false,
            error: None,
            refresh_task: None,
            api_key_input,
            view: PanelView::CollectionsList,
            selected_environment: None,
            env_vars: Arc::new(HashMap::new()),
            show_request_headers: false,
            show_request_body: true,
            show_response_headers: false,
            env_selector_handle: PopoverMenuHandle::default(),
            env_load_task: None,
            collection_load_task: None,
            request_task: None,
        };
        panel.refresh(cx);
        panel
    }

    fn api_client(&self, cx: &App) -> Option<Arc<PostmanHttpClient>> {
        let settings = &ProjectSettings::get_global(cx).postman;
        if !settings.enabled {
            return None;
        }
        let api_key = settings.api_key.clone()?;
        Some(Arc::new(PostmanHttpClient::new(
            self.http_client.clone(),
            api_key,
        )))
    }

    fn postman_configured(&self, cx: &App) -> bool {
        let s = &ProjectSettings::get_global(cx).postman;
        s.enabled && s.api_key.is_some()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.api_client(cx) else {
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
        self.view = PanelView::CollectionsList;
        self.last_response = None;
        cx.notify();

        self.collection_load_task = Some(cx.spawn(async move |this: WeakEntity<PostmanPanel>, cx| {
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
        }));
    }

    fn load_environment(&mut self, env_id: String, cx: &mut Context<Self>) {
        let Some(client) = self.api_client(cx) else {
            return;
        };
        // Keep old env_vars live until the fetch completes so the URL preview
        // doesn't briefly show unresolved {{placeholders}} during the async gap.
        self.selected_environment = Some(env_id.clone());
        cx.notify();

        // Drop any in-flight fetch so a previous selection can't overwrite the new one.
        self.env_load_task = Some(cx.spawn(async move |this: WeakEntity<PostmanPanel>, cx| {
            let result: Result<HashMap<String, String>> = cx
                .background_spawn(async move {
                    let json = client.get_json(&format!("/environments/{env_id}")).await?;
                    anyhow::Ok(parse_env_vars(&json))
                })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok(vars) => this.env_vars = Arc::new(vars),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn reset_request_view_toggles(&mut self) {
        self.show_request_headers = false;
        self.show_request_body = true;
        self.show_response_headers = false;
    }

    fn open_request_detail(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.view = PanelView::RequestDetail { request_idx: idx };
        self.last_response = None;
        self.reset_request_view_toggles();
        cx.notify();
    }

    fn back_to_list(&mut self, cx: &mut Context<Self>) {
        self.view = PanelView::CollectionsList;
        self.reset_request_view_toggles();
        cx.notify();
    }

    fn run_selected_request(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.api_client(cx) else {
            return;
        };
        let Some(detail) = &self.collection_detail else {
            return;
        };
        let PanelView::RequestDetail { request_idx: idx } = self.view else {
            return;
        };
        let Some(req) = detail.requests.get(idx) else {
            return;
        };

        let request_def = req.request_def.clone();
        let env_vars = self.env_vars.clone();
        self.last_response = None;
        self.error = None;
        cx.notify();

        self.request_task = Some(cx.spawn(async move |this: WeakEntity<PostmanPanel>, cx| {
            let result: Result<ExecutionResult> = cx
                .background_spawn(async move {
                    client.execute_request(&request_def, &env_vars).await
                })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok(r) => this.last_response = Some(r),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn save_api_key(&mut self, cx: &mut Context<Self>) {
        let api_key = self.api_key_input.read(cx).text(cx).trim().to_string();
        if api_key.is_empty() {
            return;
        }

        let fs = <dyn Fs>::global(cx);
        update_settings_file(fs, cx, move |settings, _| {
            let postman = settings.project.postman.get_or_insert_default();
            postman.api_key = Some(api_key);
            postman.enabled = Some(true);
        });

        self.error = None;
        self.refresh(cx);
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

fn status_color(status: u16) -> Color {
    if status < 300 {
        Color::Success
    } else if status < 500 {
        Color::Warning
    } else {
        Color::Error
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

/// Postman list responses include both `uid` (`{ownerId}-{id}`, required by single-resource
/// endpoints) and bare `id`. Always prefer `uid` so collection/environment fetches don't 404.
fn extract_postman_id(obj: &serde_json::Value) -> Option<String> {
    obj["uid"].as_str().or_else(|| obj["id"].as_str()).map(str::to_string)
}

fn parse_collections(json: serde_json::Value) -> Vec<CollectionSummary> {
    parse_resource_list(&json, "collections", |c| {
        Some(CollectionSummary {
            id: extract_postman_id(c)?,
            name: c["name"].as_str()?.to_string(),
        })
    })
}

fn parse_environments(json: serde_json::Value) -> Vec<EnvironmentSummary> {
    parse_resource_list(&json, "environments", |e| {
        Some(EnvironmentSummary {
            id: extract_postman_id(e)?,
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
            out.push(RequestItem {
                name,
                method,
                path: full_path,
                request_def: Arc::new(item["request"].clone()),
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
        let is_configured = self.postman_configured(cx);

        // ── Environment selector ─────────────────────────────────────────
        let selected_env_name: gpui::SharedString = self
            .selected_environment
            .as_ref()
            .and_then(|id| self.environments.iter().find(|e| &e.id == id))
            .map(|e| e.name.clone())
            .unwrap_or_else(|| "No Env".to_string())
            .into();

        let environments = self.environments.clone();
        let panel_weak = cx.weak_entity();

        let env_selector = PopoverMenu::new("postman-env-selector")
            .anchor(gpui::Corner::BottomRight)
            .menu(move |window, cx| {
                let environments = environments.clone();
                let panel_weak = panel_weak.clone();
                Some(ContextMenu::build(window, cx, move |mut menu, _, _| {
                    let pw = panel_weak.clone();
                    menu = menu.item(
                        ContextMenuEntry::new("No Environment").handler(move |_window, cx| {
                            pw.update(cx, |this, cx| {
                                this.selected_environment = None;
                                this.env_vars = Arc::new(HashMap::new());
                                cx.notify();
                            })
                            .ok();
                        }),
                    );
                    for env in &environments {
                        let env_id = env.id.clone();
                        let pw = panel_weak.clone();
                        menu = menu.item(
                            ContextMenuEntry::new(env.name.clone()).handler(move |_window, cx| {
                                let env_id = env_id.clone();
                                pw.update(cx, |this, cx| {
                                    this.load_environment(env_id, cx);
                                })
                                .ok();
                            }),
                        );
                    }
                    menu
                }))
            })
            .trigger(
                Button::new("postman-env-trigger", selected_env_name)
                    .style(ButtonStyle::Transparent)
                    .tooltip(Tooltip::text("Select environment")),
            )
            .with_handle(self.env_selector_handle.clone());

        // ── Header bar ───────────────────────────────────────────────────
        let header = h_flex()
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
            .child(env_selector)
            .child(
                Button::new("refresh-postman", "↻")
                    .style(ButtonStyle::Transparent)
                    .tooltip(Tooltip::text("Refresh collections"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh(cx);
                    })),
            );

        // ── Main content area ────────────────────────────────────────────
        let mut content =
            v_flex().id("postman-content").flex_1().overflow_y_scroll().p_2().gap_2();

        if is_loading {
            content = content.child(
                Label::new("Loading…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        } else if !is_configured && self.collections.is_empty() {
            content = content
                .child(
                    Label::new("Enter your Postman API key")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    Label::new("Get one at postman.co/settings/me/api-keys")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(self.api_key_input.clone())
                .child(
                    Button::new("save-api-key", "Save & Connect")
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.save_api_key(cx);
                        })),
                );
        } else if let Some(err) = &self.error {
            content = content.child(
                Label::new(format!("Error: {err}"))
                    .size(LabelSize::Small)
                    .color(Color::Error),
            );
        }

        match &self.view {
            PanelView::CollectionsList => {
                content = self.render_collections_list(content, is_configured, cx);
            }
            PanelView::RequestDetail { request_idx } => {
                let idx = *request_idx;
                content = self.render_request_detail(content, idx, cx);
            }
        }

        v_flex()
            .id("postman-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .child(header)
            .child(content)
    }
}

impl PostmanPanel {
    fn render_collections_list(
        &self,
        mut content: gpui::Stateful<gpui::Div>,
        is_configured: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let is_loading = self.is_loading;
        if !self.collections.is_empty() {
            content = content.child(
                Label::new("Collections")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );

            for col in &self.collections {
                let col_id = col.id.clone();
                let is_selected =
                    self.selected_collection.as_deref() == Some(col_id.as_str());
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
        } else if !is_loading && self.error.is_none() && is_configured {
            content = content.child(
                Label::new("No collections")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
        }

        if let Some(detail) = &self.collection_detail {
            content = content.child(Divider::horizontal().color(DividerColor::Border));
            content = content.child(
                Label::new("Requests")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );

            for (idx, req) in detail.requests.iter().enumerate() {
                let method_clr = method_color(&req.method);
                let row = h_flex()
                    .id(gpui::SharedString::from(format!("req_{idx}")))
                    .gap_1()
                    .cursor_pointer()
                    .px_1()
                    .rounded_md()
                    .child(
                        Label::new(req.method.chars().take(6).collect::<String>())
                            .size(LabelSize::Small)
                            .color(method_clr),
                    )
                    .child(
                        Label::new(req.name.chars().take(35).collect::<String>())
                            .size(LabelSize::Small),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_request_detail(idx, cx);
                    }));
                content = content.child(row);
            }
        }

        content
    }

    fn render_request_detail(
        &self,
        mut content: gpui::Stateful<gpui::Div>,
        idx: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let Some(detail) = &self.collection_detail else {
            return content;
        };
        let Some(req) = detail.requests.get(idx) else {
            return content;
        };

        let method_clr = method_color(&req.method);
        let resolved_url = extract_url(&req.request_def)
            .map(|u| substitute_vars(&u, &self.env_vars))
            .unwrap_or_default();

        // Back row
        content = content.child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("back-to-list", "← Back")
                        .style(ButtonStyle::Transparent)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.back_to_list(cx);
                        })),
                )
                .child(
                    Label::new(req.path.chars().take(50).collect::<String>())
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
        );

        // Method + URL
        content = content.child(
            h_flex()
                .gap_1()
                .child(
                    Label::new(req.method.clone())
                        .size(LabelSize::Small)
                        .color(method_clr),
                )
                .child(Label::new(resolved_url).size(LabelSize::Small)),
        );

        // Request headers (collapsible)
        let headers_arr = req.request_def["header"].as_array();
        if headers_arr.map_or(false, |a| !a.is_empty()) {
            let show = self.show_request_headers;
            content = content.child(
                h_flex()
                    .id("toggle-req-headers")
                    .gap_1()
                    .cursor_pointer()
                    .child(
                        Label::new(if show { "▾ Headers" } else { "▸ Headers" })
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_request_headers = !this.show_request_headers;
                        cx.notify();
                    })),
            );
            if show {
                for h in headers_arr.into_iter().flatten() {
                    let key = h["key"].as_str().unwrap_or("");
                    let val = h["value"].as_str().unwrap_or("");
                    if key.is_empty() {
                        continue;
                    }
                    let val_resolved = substitute_vars(val, &self.env_vars);
                    content = content.child(
                        Label::new(format!("{key}: {val_resolved}"))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    );
                }
            }
        }

        // Request body (collapsible)
        let body_raw = req.request_def["body"]["raw"].as_str().unwrap_or("");
        if !body_raw.is_empty() {
            let show = self.show_request_body;
            content = content.child(
                h_flex()
                    .id("toggle-req-body")
                    .gap_1()
                    .cursor_pointer()
                    .child(
                        Label::new(if show { "▾ Body" } else { "▸ Body" })
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_request_body = !this.show_request_body;
                        cx.notify();
                    })),
            );
            if show {
                let body_resolved = substitute_vars(body_raw, &self.env_vars);
                content = content.child(
                    Label::new(body_resolved)
                        .size(LabelSize::Small)
                        .color(Color::Default),
                );
            }
        }

        // Send button
        content = content.child(
            Button::new("send-request", "▶ Send")
                .style(ButtonStyle::Filled)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.run_selected_request(cx);
                })),
        );

        // Response area
        if let Some(result) = &self.last_response {
            content = content.child(Divider::horizontal().color(DividerColor::Border));

            // Status + duration
            content = content.child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new(result.status.to_string())
                            .size(LabelSize::Small)
                            .color(status_color(result.status)),
                    )
                    .child(
                        Label::new(format!("{}ms", result.duration_ms))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            );

            // Response headers (collapsible)
            if !result.headers.is_empty() {
                let show = self.show_response_headers;
                content = content.child(
                    h_flex()
                        .id("toggle-resp-headers")
                        .gap_1()
                        .cursor_pointer()
                        .child(
                            Label::new(if show {
                                "▾ Response Headers"
                            } else {
                                "▸ Response Headers"
                            })
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_response_headers = !this.show_response_headers;
                            cx.notify();
                        })),
                );
                if show {
                    for (k, v) in &result.headers {
                        content = content.child(
                            Label::new(format!("{k}: {v}"))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        );
                    }
                }
            }

            // Full response body, no truncation
            content = content.child(
                Label::new("Response Body")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            );
            content = content.child(Label::new(result.body.clone()).size(LabelSize::Small));
        }

        content
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
