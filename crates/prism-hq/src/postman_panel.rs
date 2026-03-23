use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use futures::AsyncReadExt as _;
use fs::Fs;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Render,
    Subscription, Task, WeakEntity, Window, actions, px,
};
use http_client::{AsyncBody, HttpClientWithUrl, Method};
use project::project_settings::ProjectSettings;
use settings::{Settings as _, update_settings_file};
use ui::{
    Button, ButtonStyle, Callout, Chip, Color, ContextMenu, ContextMenuEntry, Divider, DividerColor,
    Icon, IconButton, IconName, Label, LabelSize, ListItem, PopoverMenu, PopoverMenuHandle,
    Severity, SpinnerLabel, Tooltip, h_flex, prelude::*, v_flex,
};
use ui_input::{ErasedEditorEvent, InputField};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(prism_hq, [TogglePostmanPanel]);

const POSTMAN_PANEL_KEY: &str = "postman_panel";

// ── Lightweight Postman API client ────────────────────────────────────────

pub(crate) struct PostmanHttpClient {
    http_client: Arc<HttpClientWithUrl>,
    api_key: String,
}

impl PostmanHttpClient {
    pub(crate) fn new(http_client: Arc<HttpClientWithUrl>, api_key: String) -> Self {
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

    /// Execute a request using a pre-resolved URL string (e.g. from the editable URL bar).
    pub(crate) async fn execute_request_with_url(
        &self,
        url: &str,
        request_def: &serde_json::Value,
        env_vars: &HashMap<String, String>,
    ) -> Result<ExecutionResult> {
        let method_str = request_def["method"]
            .as_str()
            .unwrap_or("GET")
            .to_uppercase();
        let url = url.to_string();

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
pub(crate) fn extract_url(request_def: &serde_json::Value) -> Option<String> {
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
            .filter_map(|p| {
                Some(format!(
                    "{}={}",
                    urlencoding::encode(p["key"].as_str()?),
                    urlencoding::encode(p["value"].as_str()?)
                ))
            })
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

pub(crate) fn substitute_vars(s: &str, vars: &HashMap<String, String>) -> String {
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

/// Return the names of any `{{variable}}` placeholders remaining after substitution —
/// i.e. variables referenced in `s` that are not present in `vars`.
pub(crate) fn find_unresolved_vars(s: &str, vars: &HashMap<String, String>) -> Vec<String> {
    if !s.contains("{{") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        rest = &rest[start + 2..];
        if let Some(end) = rest.find("}}") {
            let name = rest[..end].trim();
            if !name.is_empty() && !vars.contains_key(name) {
                out.push(name.to_string());
            }
            rest = &rest[end + 2..];
        } else {
            break;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
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
            // Use unwrap_or so vars with an explicit empty value ("") are preserved —
            // ?-unwrapping would silently drop them since as_str() returns None for null.
            let val = v["value"].as_str().unwrap_or("");
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
pub(crate) struct RequestItem {
    pub(crate) name: String,
    pub(crate) method: String,
    /// Full path within the collection, e.g. "Folder/Request Name".
    pub(crate) path: String,
    /// Arc-wrapped so cloning a RequestItem (or the entire CollectionDetail) is cheap,
    /// and the background-task move in run_request doesn't deep-copy the JSON.
    pub(crate) request_def: Arc<serde_json::Value>,
    /// Pre-computed lowercase versions used in search filtering, avoiding per-keystroke allocs.
    pub(crate) name_lower: String,
    pub(crate) path_lower: String,
}

/// A node in the collection tree — either a folder (with nested children) or a leaf request.
#[derive(Debug, Clone)]
enum CollectionNode {
    Folder {
        name: String,
        /// Full slash-delimited path within the collection (e.g. "Auth/Tokens").
        /// Used as the `expanded_folders` key so same-named folders at the same depth
        /// under different parents don't share state.
        path: String,
        children: Vec<CollectionNode>,
    },
    /// Index into `CollectionDetail.requests` for the flat list.
    Request(usize),
}

#[derive(Debug, Clone)]
struct CollectionDetail {
    /// Tree structure preserving the folder hierarchy from Postman.
    nodes: Vec<CollectionNode>,
    /// Flat list for O(1) index lookup.
    requests: Vec<RequestItem>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExecutionResult {
    pub(crate) status: u16,
    pub(crate) duration_ms: u128,
    /// Response headers sorted by name — sorted once at creation, never re-sorted in render.
    pub(crate) headers: Vec<(String, String)>,
    /// Arc-backed so render clones are cheap regardless of response size.
    pub(crate) body: gpui::SharedString,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RequestTab {
    Params,
    Headers,
    Body,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ResponseTab {
    Body,
    Headers,
}

// ── Panel ─────────────────────────────────────────────────────────────────

pub struct PostmanPanel {
    focus_handle: FocusHandle,
    position: DockPosition,
    width: Option<gpui::Pixels>,
    http_client: Arc<HttpClientWithUrl>,
    workspace: Option<WeakEntity<Workspace>>,

    collections: Vec<CollectionSummary>,
    environments: Vec<EnvironmentSummary>,
    selected_collection: Option<String>,
    collection_detail: Option<CollectionDetail>,
    is_refreshing: bool,
    is_loading_detail: bool,
    error: Option<String>,
    refresh_task: Option<Task<()>>,
    api_key_input: Entity<InputField>,
    search_input: Entity<InputField>,
    /// Folder paths that are currently expanded in the tree view.
    expanded_folders: HashSet<String>,

    selected_environment: Option<String>,
    env_vars: Entity<HashMap<String, String>>,
    env_selector_handle: PopoverMenuHandle<ContextMenu>,
    env_load_task: Option<Task<()>>,
    collection_load_task: Option<Task<()>>,
    // Keep alive so the search bar triggers re-renders on every keystroke.
    _search_subscription: Subscription,
}

impl EventEmitter<PanelEvent> for PostmanPanel {}

impl PostmanPanel {
    pub fn new(
        http_client: Arc<HttpClientWithUrl>,
        workspace: Option<WeakEntity<Workspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let api_key_input = cx.new(|cx| InputField::new(window, cx, "PMAK-xxxxxxxx…"));
        let search_input = cx.new(|cx| InputField::new(window, cx, "Search…"));
        // Subscribe to the underlying editor directly so only actual text edits (BufferEdited)
        // trigger a re-render — not cursor moves or selection changes.
        let search_editor = search_input.read(cx).editor().clone();
        let panel_weak = cx.weak_entity();
        let search_sub = search_editor.subscribe(
            Box::new(move |event, _window, cx| {
                if event == ErasedEditorEvent::BufferEdited {
                    panel_weak.update(cx, |_, cx| cx.notify()).ok();
                }
            }),
            window,
            cx,
        );

        let mut panel = Self {
            focus_handle,
            position: DockPosition::Right,
            width: None,
            http_client,
            workspace,
            collections: Vec::new(),
            environments: Vec::new(),
            selected_collection: None,
            collection_detail: None,
            is_refreshing: false,
            is_loading_detail: false,
            error: None,
            refresh_task: None,
            api_key_input,
            search_input,
            expanded_folders: HashSet::new(),
            selected_environment: None,
            env_vars: cx.new(|_| HashMap::new()),
            env_selector_handle: PopoverMenuHandle::default(),
            env_load_task: None,
            collection_load_task: None,
            _search_subscription: search_sub,
        };
        panel.refresh(cx);
        panel
    }

    fn api_client(&self, cx: &App) -> Option<Arc<PostmanHttpClient>> {
        let settings = &ProjectSettings::get_global(cx).postman;
        if !settings.enabled {
            log::debug!("api_client: postman integration disabled");
            return None;
        }
        if settings.api_key.is_none() {
            log::debug!("api_client: no API key configured");
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

        self.is_refreshing = true;
        self.error = None;
        self.collection_detail = None;
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
                this.is_refreshing = false;
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
        // Always update UI state first — user must see immediate feedback.
        self.selected_collection = Some(collection_id.clone());
        self.collection_detail = None;
        self.expanded_folders.clear();
        self.error = None;

        let Some(client) = self.api_client(cx) else {
            log::warn!("load_collection: no API client (key missing or disabled)");
            self.error = Some("Postman API key not configured".into());
            cx.notify();
            return;
        };

        log::info!("load_collection: fetching {}", collection_id);
        self.is_loading_detail = true;
        cx.notify();

        let collection_id_for_guard = collection_id.clone();
        self.collection_load_task = Some(cx.spawn(async move |this: WeakEntity<PostmanPanel>, cx| {
            let result: Result<CollectionDetail> = cx
                .background_spawn(async move {
                    let path = format!("/collections/{collection_id}");
                    log::info!("load_collection: GET https://api.postman.com{path}");
                    let json = client.get_json(&path).await?;
                    log::info!(
                        "load_collection: response top-level keys: {:?}",
                        json.as_object().map(|o| o.keys().collect::<Vec<_>>())
                    );
                    anyhow::Ok(parse_collection_detail(&json))
                })
                .await;

            this.update(cx, |this, cx| {
                // Guard against a race: user deselected or clicked a different collection
                // while this fetch was in flight. Discard the stale result either way.
                if this.selected_collection.as_deref() != Some(&collection_id_for_guard) {
                    this.is_loading_detail = false;
                    cx.notify();
                    return;
                }
                this.is_loading_detail = false;
                match result {
                    Ok(detail) => {
                        log::info!(
                            "load_collection: fetched {} requests",
                            detail.requests.len()
                        );
                        this.collection_detail = Some(detail);
                    }
                    Err(e) => {
                        log::error!("load_collection: fetch failed: {e}");
                        this.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn load_environment(&mut self, env_id: String, cx: &mut Context<Self>) {
        // Update selection immediately so the UI reflects the choice.
        // Keep old env_vars live until the fetch completes so the URL preview
        // doesn't briefly show unresolved {{placeholders}} during the async gap.
        self.selected_environment = Some(env_id.clone());
        self.error = None;

        let Some(client) = self.api_client(cx) else {
            log::debug!("load_environment: no API client available (key missing or disabled)");
            self.error = Some("Postman API key not configured".into());
            self.env_load_task = None;
            cx.notify();
            return;
        };

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
                    Ok(vars) => {
                        this.env_vars.update(cx, |ev, cx| {
                            *ev = vars;
                            cx.notify();
                        });
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Open a request as a workspace tab. Falls back silently if no workspace is available
    /// (e.g. panel opened outside a workspace context).
    fn open_request_detail(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let req = match self.collection_detail.as_ref().and_then(|d| d.requests.get(idx)) {
            Some(r) => r.clone(),
            None => return,
        };
        let Some(ws) = self.workspace.as_ref().and_then(|w| w.upgrade()) else {
            return;
        };
        let Some(col_id) = self.selected_collection.clone() else {
            // selected_collection is always set before collection_detail is populated,
            // so this path is unreachable in practice.
            return;
        };
        let col_name = self.selected_collection_name().to_string();
        let http_client = self.http_client.clone();
        let env_vars = self.env_vars.clone();
        let language_registry = ws.read(cx).app_state().languages.clone();
        ws.update(cx, |workspace, cx| {
            crate::postman_request_item::open_postman_request(
                workspace, req, col_id, col_name, http_client, env_vars, language_registry, window, cx,
            );
        });
    }

    fn deselect_collection(&mut self, cx: &mut Context<Self>) {
        self.selected_collection = None;
        self.collection_detail = None;
        self.expanded_folders.clear();
        self.is_loading_detail = false;
        self.collection_load_task = None;
        self.error = None;
        cx.notify();
    }

    fn selected_collection_name(&self) -> gpui::SharedString {
        self.selected_collection
            .as_deref()
            .and_then(|id| self.collections.iter().find(|c| c.id == id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "Collection".into())
            .into()
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

pub(crate) fn method_color(method: &str) -> Color {
    match method {
        "GET" => Color::Success,
        "POST" => Color::Accent,
        "PUT" | "PATCH" => Color::Warning,
        "DELETE" => Color::Error,
        _ => Color::Default,
    }
}

pub(crate) fn status_color(status: u16) -> Color {
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

    let nodes = if let Some(items) = items {
        build_collection_nodes(items, "", &mut requests)
    } else {
        Vec::new()
    };

    CollectionDetail { nodes, requests }
}

fn build_collection_nodes(
    items: &[serde_json::Value],
    prefix: &str,
    flat: &mut Vec<RequestItem>,
) -> Vec<CollectionNode> {
    build_collection_nodes_inner(items, prefix, flat, 0)
}

fn build_collection_nodes_inner(
    items: &[serde_json::Value],
    prefix: &str,
    flat: &mut Vec<RequestItem>,
    depth: usize,
) -> Vec<CollectionNode> {
    // Postman doesn't support arbitrarily deep nesting in practice, but guard
    // against malformed exports forming cycles or pathologically deep trees.
    if depth > 20 {
        return Vec::new();
    }
    let mut nodes = Vec::new();
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
            let idx = flat.len();
            flat.push(RequestItem {
                name_lower: name.to_lowercase(),
                path_lower: full_path.to_lowercase(),
                name: name.clone(),
                method,
                path: full_path,
                request_def: Arc::new(item["request"].clone()),
            });
            nodes.push(CollectionNode::Request(idx));
        } else if let Some(children) = item["item"].as_array() {
            let child_nodes = build_collection_nodes_inner(children, &full_path, flat, depth + 1);
            nodes.push(CollectionNode::Folder {
                name,
                path: full_path,
                children: child_nodes,
            });
        }
    }
    nodes
}

/// Recursively render tree nodes into a flat element list.
/// Takes individual fields rather than `&PostmanPanel` to avoid borrow conflicts
/// with the caller's existing borrows on `collection_detail`.
fn render_collection_nodes(
    nodes: &[CollectionNode],
    requests: &[RequestItem],
    expanded_folders: &HashSet<String>,
    depth: usize,
    elements: &mut Vec<gpui::AnyElement>,
    cx: &mut Context<PostmanPanel>,
) {
    for node in nodes {
        match node {
            CollectionNode::Folder { name, path, children } => {
                // Key by full path so same-named folders under different parents stay independent.
                let expanded = expanded_folders.contains(path);
                let fk = path.clone();
                let item = ListItem::new(format!("folder-{path}"))
                    .indent_level(depth)
                    .toggle(Some(expanded))
                    .start_slot(
                        Icon::new(if expanded {
                            IconName::FolderOpen
                        } else {
                            IconName::Folder
                        })
                        .size(ui::IconSize::Small)
                        .color(Color::Muted),
                    )
                    .child(Label::new(name.clone()).size(LabelSize::Small))
                    .on_toggle(cx.listener(move |this, _, _, cx| {
                        if this.expanded_folders.contains(&fk) {
                            this.expanded_folders.remove(&fk);
                        } else {
                            this.expanded_folders.insert(fk.clone());
                        }
                        cx.notify();
                    }));
                elements.push(item.into_any_element());
                if expanded {
                    render_collection_nodes(
                        children,
                        requests,
                        expanded_folders,
                        depth + 1,
                        elements,
                        cx,
                    );
                }
            }
            CollectionNode::Request(idx) => {
                let idx = *idx;
                if let Some(req) = requests.get(idx) {
                    let method_clr = method_color(&req.method);
                    let url_preview = extract_url(&req.request_def).unwrap_or_default();
                    let item = ListItem::new(format!("req-{idx}"))
                        .indent_level(depth)
                        .start_slot(
                            Chip::new(req.method.clone())
                                .label_color(method_clr)
                                .tooltip(Tooltip::text(format!("{} request", req.method))),
                        )
                        .child(Label::new(req.name.clone()).size(LabelSize::Small).truncate())
                        .tooltip(Tooltip::text(url_preview))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_request_detail(idx, window, cx);
                        }));
                    elements.push(item.into_any_element());
                }
            }
        }
    }
}

/// Render a key-value row (used for headers and params).
pub(crate) fn kv_row(key: &str, value: &str) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(Label::new(key.to_string()).size(LabelSize::Small).color(Color::Muted))
        .child(Label::new(value.to_string()).size(LabelSize::Small))
}

impl Focusable for PostmanPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PostmanPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_refreshing = self.is_refreshing;
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
                                this.env_vars.update(cx, |ev, cx| {
                                    ev.clear();
                                    cx.notify();
                                });
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
            .child(div().flex_1())
            .child(env_selector)
            .child(
                Button::new("refresh-postman", "↻")
                    .style(ButtonStyle::Transparent)
                    .tooltip(Tooltip::text("Refresh collections"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh(cx);
                    })),
            );

        // ── Search bar ───────────────────────────────────────────────────
        let show_search = is_configured && !is_refreshing;
        let search_bar = show_search.then(|| {
            h_flex()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(cx.theme().colors().border)
                .child(self.search_input.clone())
        });

        // ── Main content area ────────────────────────────────────────────
        let mut content =
            v_flex().id("postman-content").flex_1().overflow_y_scroll().p_2().gap_2();

        if is_refreshing {
            content = content.child(
                h_flex()
                    .gap_1()
                    .child(SpinnerLabel::new())
                    .child(
                        Label::new("Connecting to Postman…")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
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
        }

        // Error always visible, regardless of loading state
        if let Some(err) = &self.error {
            content = content.child(
                Label::new(format!("Error: {err}"))
                    .size(LabelSize::Small)
                    .color(Color::Error),
            );
        }

        if !is_refreshing {
            content = self.render_collections_list(content, is_configured, cx);
        }

        v_flex()
            .id("postman-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .child(header)
            .children(search_bar)
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
        let is_loading_detail = self.is_loading_detail;
        let query = self.search_input.read(cx).text(cx).trim().to_lowercase();

        if self.collection_detail.is_some() || is_loading_detail {
            // ── Breadcrumb: Collections > CollectionName ──────────────────
            let col_name = self.selected_collection_name();

            content = content.child(
                h_flex()
                    .id("breadcrumb-nav")
                    .gap_1()
                    .items_center()
                    .child(
                        IconButton::new("back-to-collections", IconName::ArrowLeft)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.deselect_collection(cx);
                            }))
                            .tooltip(Tooltip::text("Back to Collections")),
                    )
                    .child(Label::new("Collections").size(LabelSize::Small).color(Color::Muted))
                    .child(Label::new("›").size(LabelSize::Small).color(Color::Muted))
                    .child(
                        Label::new(col_name)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    ),
            );

            if is_loading_detail && self.collection_detail.is_none() {
                content = content.child(
                    h_flex()
                        .gap_1()
                        .child(SpinnerLabel::new())
                        .child(
                            Label::new("Loading requests…")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                );
            }

            if let Some(detail) = &self.collection_detail {
                content = content.child(Divider::horizontal().color(DividerColor::Border));

                if detail.requests.is_empty() {
                    content = content.child(
                        Callout::new()
                            .severity(Severity::Info)
                            .icon(IconName::Folder)
                            .title("Empty collection")
                            .description("No requests found. This collection may not have been exported with request items."),
                    );
                } else if !query.is_empty() {
                    // Search active: flat filtered list, no tree structure
                    let mut any = false;
                    for (idx, req) in detail.requests.iter().enumerate() {
                        if req.name_lower.contains(&query) || req.path_lower.contains(&query) {
                            any = true;
                            let method_clr = method_color(&req.method);
                            let row = ListItem::new(format!("req-search-{idx}"))
                            .start_slot(Chip::new(req.method.clone()).label_color(method_clr))
                            .child(
                                Label::new(req.name.clone())
                                    .size(LabelSize::Small)
                                    .truncate(),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_request_detail(idx, window, cx);
                            }));
                            content = content.child(row);
                        }
                    }
                    if !any {
                        content = content.child(
                            Label::new("No matching requests")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        );
                    }
                } else {
                    // No search: render the folder tree
                    let nodes = &detail.nodes;
                    let requests = &detail.requests;
                    let expanded_folders = &self.expanded_folders;
                    let mut elements: Vec<gpui::AnyElement> = Vec::new();
                    render_collection_nodes(nodes, requests, expanded_folders, 0, &mut elements, cx);
                    for elem in elements {
                        content = content.child(elem);
                    }
                }
            }
        } else {
            // No collection selected — show top-level collection list with optional filter.
            if !self.collections.is_empty() {
                content = content.child(
                    Label::new("Collections")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                );

                for col in &self.collections {
                    if !query.is_empty() && !col.name.to_lowercase().contains(&query) {
                        continue;
                    }
                    let col_id = col.id.clone();
                    let col_name_tip = col.name.clone();
                    let row = h_flex()
                        .id(format!("col_{col_id}"))
                        .gap_1()
                        .cursor_pointer()
                        .px_1()
                        .rounded_md()
                        .hover(|style| style.bg(cx.theme().colors().element_hover))
                        .tooltip(Tooltip::text(format!("Browse requests in \"{}\"", col_name_tip)))
                        .child(
                            Icon::new(IconName::Folder)
                                .size(ui::IconSize::Small)
                                .color(Color::Muted),
                        )
                        .child(Label::new(col.name.clone()).size(LabelSize::Small).truncate())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.load_collection(col_id.clone(), cx);
                        }));
                    content = content.child(row);
                }
            } else if self.error.is_none() && is_configured {
                content = content.child(
                    Callout::new()
                        .severity(Severity::Info)
                        .icon(IconName::Folder)
                        .title("No collections found")
                        .description("Hit ↻ to refresh or check your API key in settings."),
                );
            }
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
