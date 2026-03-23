use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use util::size::format_file_size;

use anyhow::Result;
use editor::{Editor, EditorMode};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, KeyBinding,
    Render, SharedString, Subscription, Task, WeakEntity, Window, actions,
};
use http_client::HttpClientWithUrl;
use language::{Buffer, LanguageRegistry};
use multi_buffer::MultiBuffer;
use project::project_settings::ProjectSettings;
use settings::Settings as _;
use ui::{
    Banner, Button, ButtonStyle, Callout, Chip, Color, CopyButton, Divider, DividerColor, Icon,
    IconName, Indicator, Label, LabelSize, Severity, Tab, TabBar, Tooltip, h_flex, prelude::*,
    v_flex,
};
use ui_input::InputField;
use workspace::Workspace;
use workspace::item::{Item, ItemEvent, TabContentParams};

use crate::postman_panel::{
    ExecutionResult, PostmanHttpClient, RequestItem, RequestTab, ResponseTab, extract_url,
    find_unresolved_vars, kv_row, method_color, status_color, substitute_vars,
};

actions!(prism_hq, [SendPostmanRequest, CancelPostmanRequest, FocusPostmanUrlBar]);

// Displayed in button tooltips — must stay in sync with register_keybindings().
const SEND_KEYBINDING_HINT: &str = "⌘↩";
const CANCEL_KEYBINDING_HINT: &str = "Esc";
const FOCUS_URL_KEYBINDING_HINT: &str = "⌘L";

pub struct PostmanRequestItem {
    focus_handle: FocusHandle,
    /// Owns name, method, path, and request_def — avoids duplicating these fields.
    request: RequestItem,
    /// Postman collection UID — used as the tab dedup key so same-named collections don't collide.
    collection_id: String,
    collection_name: String,
    http_client: Arc<HttpClientWithUrl>,
    /// Shared with the panel so env changes propagate to open tabs reactively.
    env_vars: Entity<HashMap<String, String>>,
    active_request_tab: RequestTab,
    active_response_tab: ResponseTab,
    last_response: Option<ExecutionResult>,
    error: Option<String>,
    request_task: Option<Task<()>>,
    /// Tick task drives 100ms re-renders for the elapsed timer while a request is in flight.
    tick_task: Option<Task<()>>,
    send_start: Option<Instant>,
    url_input: Entity<InputField>,
    response_editor: Entity<Editor>,
    body_editor: Entity<Editor>,
    language_registry: Arc<LanguageRegistry>,
    /// Keeps the env observation alive for the lifetime of this tab.
    _env_sub: Subscription,
}

impl PostmanRequestItem {
    pub(crate) fn new(
        request: RequestItem,
        collection_id: String,
        collection_name: String,
        http_client: Arc<HttpClientWithUrl>,
        env_vars: Entity<HashMap<String, String>>,
        language_registry: Arc<LanguageRegistry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let resolved_url = extract_url(&request.request_def)
            .map(|u| substitute_vars(&u, &env_vars.read(cx)))
            .unwrap_or_default();

        let url_input = cx.new(|cx| {
            let field = InputField::new(window, cx, "https://…");
            field.set_text(&resolved_url, window, cx);
            field
        });

        let response_editor =
            make_readonly_editor(String::new(), 50, window, cx);

        // Body editor — read-only with syntax highlighting; initialized with the raw body text.
        let body_raw = request.request_def["body"]["raw"].as_str().unwrap_or("").to_string();
        let body_editor =
            make_readonly_editor(body_raw.clone(), 30, window, cx);

        // Assign JSON language if the body looks like JSON.
        let body_trimmed = body_raw.trim_start();
        let body_looks_json = body_trimmed.starts_with('{') || body_trimmed.starts_with('[');
        if body_looks_json {
            let lang_reg = language_registry.clone();
            let editor = body_editor.clone();
            cx.spawn(async move |_, cx| {
                if let Ok(json_lang) = lang_reg.language_for_name("JSON").await {
                    cx.update(|cx| {
                        editor.update(cx, |ed, cx| {
                            if let Some(buf) = ed.buffer().read(cx).as_singleton() {
                                buf.update(cx, |buf, cx| buf.set_language(Some(json_lang), cx));
                            }
                        });
                    });
                }
            })
            .detach();
        }

        // Re-render when env changes so params/headers/body show updated resolved values.
        let env_sub = cx.observe(&env_vars, |_, _, cx| cx.notify());

        Self {
            focus_handle,
            request,
            collection_id,
            collection_name,
            http_client,
            env_vars,
            active_request_tab: RequestTab::Body,
            active_response_tab: ResponseTab::Body,
            last_response: None,
            error: None,
            request_task: None,
            tick_task: None,
            send_start: None,
            url_input,
            response_editor,
            body_editor,
            language_registry,
            _env_sub: env_sub,
        }
    }

    fn run_request(&mut self, cx: &mut Context<Self>) {
        let settings = ProjectSettings::get_global(cx);
        let api_key = match settings.postman.api_key.clone() {
            Some(k) => k,
            None => {
                self.error = Some("Postman API key not configured".into());
                cx.notify();
                return;
            }
        };

        let client = Arc::new(PostmanHttpClient::new(self.http_client.clone(), api_key));
        // Use the URL from the editable input bar so the user can override it.
        let url = self.url_input.read(cx).text(cx).trim().to_string();
        let request_def = self.request.request_def.clone();
        let env_vars = self.env_vars.read(cx).clone();
        self.last_response = None;
        self.error = None;
        self.send_start = Some(Instant::now());
        cx.notify();

        // 100ms tick task: drives re-renders for the elapsed timer display.
        let executor = cx.background_executor().clone();
        self.tick_task = Some(cx.spawn({
            let tick_executor = executor.clone();
            async move |this: WeakEntity<PostmanRequestItem>, cx| {
                loop {
                    tick_executor.timer(Duration::from_millis(100)).await;
                    let still_sending = this
                        .update(cx, |this, cx| {
                            if this.request_task.is_some() {
                                cx.notify();
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if !still_sending {
                        break;
                    }
                }
            }
        }));

        self.request_task = Some(cx.spawn(async move |this: WeakEntity<PostmanRequestItem>, cx| {
            let req_fut = cx.background_spawn(async move {
                client.execute_request_with_url(&url, &request_def, &env_vars).await
            });
            let timeout_fut = executor.timer(Duration::from_secs(30));

            let result: Result<ExecutionResult> =
                match futures::future::select(Box::pin(req_fut), Box::pin(timeout_fut)).await {
                    futures::future::Either::Left((res, _)) => res,
                    futures::future::Either::Right(_) => {
                        Err(anyhow::anyhow!("Request timed out after 30s"))
                    }
                };

            this.update(cx, |this, cx| {
                match result {
                    Ok(r) => {
                        // SharedString is Arc<str>-backed; clone avoids re-allocation.
                        let body = r.body.clone();
                        if let Some(buf) = this.response_editor.read(cx).buffer().read(cx).as_singleton() {
                            buf.update(cx, |buf, cx| buf.set_text(body, cx));
                        }

                        // Choose language from Content-Type header; fall back to JSON body detection.
                        let lang_name = r.headers.iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                            .and_then(|(_, v)| {
                                if v.contains("json") { Some("JSON") }
                                else if v.contains("xml") { Some("XML") }
                                else if v.contains("html") { Some("HTML") }
                                else { None }
                            });
                        if let Some(lang_name) = lang_name {
                            let lang_reg = this.language_registry.clone();
                            let editor = this.response_editor.clone();
                            let lang_name = lang_name.to_string();
                            cx.spawn(async move |_this, cx| {
                                if let Ok(lang) = lang_reg.language_for_name(&lang_name).await {
                                    cx.update(|cx| {
                                        editor.update(cx, |ed, cx| {
                                            if let Some(buf) = ed.buffer().read(cx).as_singleton() {
                                                buf.update(cx, |buf, cx| buf.set_language(Some(lang), cx));
                                            }
                                        });
                                    });
                                }
                            })
                            .detach();
                        }

                        this.last_response = Some(r);
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                this.request_task = None;
                this.tick_task = None;
                this.send_start = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn cancel_request(&mut self, cx: &mut Context<Self>) {
        if self.request_task.is_none() {
            return;
        }
        // Dropping the task cancels the in-flight request future.
        self.request_task = None;
        self.tick_task = None;
        self.send_start = None;
        self.error = Some("Request cancelled".into());
        cx.notify();
    }
}

impl EventEmitter<ItemEvent> for PostmanRequestItem {}

impl Focusable for PostmanRequestItem {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for PostmanRequestItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!("{} {}", self.request.method, self.request.name).into()
    }

    fn tab_content(&self, params: TabContentParams, _window: &Window, _cx: &App) -> AnyElement {
        h_flex()
            .gap_1()
            .child(
                Chip::new(self.request.method.clone())
                    .label_color(method_color(&self.request.method)),
            )
            .child(Label::new(self.request.name.clone()).color(params.text_color()))
            .into_any_element()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<ui::Icon> {
        Some(Icon::new(IconName::Link).color(method_color(&self.request.method)))
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        // Read from the live input field so edits are reflected in the tooltip.
        let url = self.url_input.read(cx).text(cx);
        Some(format!("{} {}", self.request.method, url.trim()).into())
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

impl Render for PostmanRequestItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let method_clr = method_color(&self.request.method);
        let env_vars = self.env_vars.read(cx).clone();
        // Trim so the copy button and unresolved-var check are consistent with what's sent.
        let url_text: SharedString = self.url_input.read(cx).text(cx).trim().to_string().into();

        // ── Unresolved variables check ────────────────────────────────────
        // Scan URL + header values + raw body for placeholders that weren't substituted.
        let mut unresolved: Vec<String> = find_unresolved_vars(&url_text, &env_vars);
        if let Some(headers) = self.request.request_def["header"].as_array() {
            for h in headers {
                let val = h["value"].as_str().unwrap_or("");
                unresolved.extend(find_unresolved_vars(val, &env_vars));
            }
        }
        if let Some(body_raw) = self.request.request_def["body"]["raw"].as_str() {
            unresolved.extend(find_unresolved_vars(body_raw, &env_vars));
        }
        unresolved.sort_unstable();
        unresolved.dedup();

        let mut content = v_flex()
            .id("postman-request-content")
            .key_context("PostmanRequest")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .overflow_y_scroll()
            .p_2()
            .gap_2()
            // Keyboard shortcuts
            .on_action(cx.listener(|this, _: &SendPostmanRequest, _, cx| {
                if this.request_task.is_none() {
                    this.run_request(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CancelPostmanRequest, _, cx| {
                this.cancel_request(cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPostmanUrlBar, w, cx| {
                let handle = this.url_input.read(cx).focus_handle(cx);
                w.focus(&handle, cx);
            }));

        // ── URL bar ───────────────────────────────────────────────────────
        content = content.child(
            h_flex()
                .px_2()
                .py_1()
                .gap_2()
                .bg(cx.theme().colors().editor_background)
                .rounded_md()
                .border_1()
                .border_color(cx.theme().colors().border)
                .child(
                    Chip::new(self.request.method.clone())
                        .label_color(method_clr)
                        .tooltip(Tooltip::text(format!(
                            "{} request",
                            self.request.method
                        ))),
                )
                .child(self.url_input.clone())
                .child(CopyButton::new("copy-url", url_text.clone())),
        );

        // ── Unresolved variables banner ───────────────────────────────────
        if !unresolved.is_empty() {
            let names = unresolved.iter().map(|v| format!("{{{{{v}}}}}")).collect::<Vec<_>>().join(", ");
            content = content.child(
                Banner::new()
                    .severity(Severity::Warning)
                    .children([Label::new(format!(
                        "Unresolved variables: {names} — select an environment or check variable names"
                    ))
                    .size(LabelSize::Small)]),
            );
        }

        // ── Request tabs ─────────────────────────────────────────────────
        let active_req_tab = self.active_request_tab;
        content = content.child(
            TabBar::new("request-tabs")
                .child(
                    Tab::new("req-tab-params")
                        .toggle_state(active_req_tab == RequestTab::Params)
                        .child(Label::new("Params").size(LabelSize::Small))
                        .tooltip(Tooltip::text("Query parameters for this request"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.active_request_tab != RequestTab::Params {
                                this.active_request_tab = RequestTab::Params;
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Tab::new("req-tab-headers")
                        .toggle_state(active_req_tab == RequestTab::Headers)
                        .child(Label::new("Headers").size(LabelSize::Small))
                        .tooltip(Tooltip::text("Request headers"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.active_request_tab != RequestTab::Headers {
                                this.active_request_tab = RequestTab::Headers;
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Tab::new("req-tab-body")
                        .toggle_state(active_req_tab == RequestTab::Body)
                        .child(Label::new("Body").size(LabelSize::Small))
                        .tooltip(Tooltip::text("Request body payload"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.active_request_tab != RequestTab::Body {
                                this.active_request_tab = RequestTab::Body;
                                cx.notify();
                            }
                        })),
                ),
        );

        // ── Active request tab content ────────────────────────────────────
        match self.active_request_tab {
            RequestTab::Params => {
                // Prefer the structured query array — it's present in all modern Postman exports.
                // Only fall back to parsing the raw URL string for legacy requests that omit it.
                let from_json: Vec<(String, String)> = self.request.request_def["url"]["query"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .filter(|p| p["disabled"].as_bool() != Some(true))
                    .filter_map(|p| {
                        let k = p["key"].as_str()?.to_string();
                        let v = p["value"].as_str().unwrap_or("").to_string();
                        Some((k, v))
                    })
                    .collect();

                let all_params = if !from_json.is_empty() {
                    from_json
                } else {
                    let url_raw = extract_url(&self.request.request_def).unwrap_or_default();
                    url_raw
                        .find('?')
                        .map(|pos| &url_raw[pos + 1..])
                        .unwrap_or("")
                        .split('&')
                        .filter(|p| !p.is_empty())
                        .filter_map(|p| {
                            let mut parts = p.splitn(2, '=');
                            let k = parts.next()?.to_string();
                            let v = parts.next().unwrap_or("").to_string();
                            Some((k, v))
                        })
                        .collect()
                };

                if all_params.is_empty() {
                    content = content.child(
                        Callout::new()
                            .severity(Severity::Info)
                            .icon(IconName::Info)
                            .title("No query parameters")
                            .description("Edit the URL above to add some (e.g. ?key=value)."),
                    );
                } else {
                    for (k, v) in &all_params {
                        let v_resolved = substitute_vars(v, &env_vars);
                        content = content.child(kv_row(k, &v_resolved));
                    }
                }
            }
            RequestTab::Headers => {
                let headers_arr = self.request.request_def["header"].as_array();
                match headers_arr {
                    Some(headers) if !headers.is_empty() => {
                        for h in headers {
                            let key = h["key"].as_str().unwrap_or("");
                            let val = h["value"].as_str().unwrap_or("");
                            if key.is_empty() {
                                continue;
                            }
                            let val_resolved = substitute_vars(val, &env_vars);
                            content = content.child(kv_row(key, &val_resolved));
                        }
                    }
                    _ => {
                        content = content.child(
                            Callout::new()
                                .severity(Severity::Info)
                                .icon(IconName::Info)
                                .title("No custom headers")
                                .description("Headers like Content-Type are added automatically when you send."),
                        );
                    }
                }
            }
            RequestTab::Body => {
                let body_raw = self.request.request_def["body"]["raw"].as_str().unwrap_or("");
                if body_raw.is_empty() {
                    content = content.child(
                        Callout::new()
                            .severity(Severity::Info)
                            .icon(IconName::Info)
                            .title("No request body")
                            .description("Typical for GET requests. POST/PUT requests usually include a JSON body."),
                    );
                } else {
                    content = content.child(
                        div()
                            .id("body-editor")
                            .p_2()
                            .bg(cx.theme().colors().editor_background)
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .max_h(gpui::px(300.))
                            .overflow_y_scroll()
                            .child(self.body_editor.clone()),
                    );
                }
            }
        }

        // ── Send / Cancel button row ──────────────────────────────────────
        let is_sending = self.request_task.is_some();
        let elapsed_label = self.send_start.map(|start| {
            let ms = start.elapsed().as_millis();
            format!("{:.1}s…", ms as f64 / 1000.0)
        });

        content = content.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new("send-request", if is_sending { "Sending…" } else { "▶ Send" })
                        .style(ButtonStyle::Filled)
                        .disabled(is_sending)
                        .tooltip(Tooltip::text(format!("Execute this request ({SEND_KEYBINDING_HINT})")))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_request(cx);
                        })),
                )
                .when(is_sending, |this| {
                    this.child(
                        Button::new("cancel-request", "✕ Cancel")
                            .style(ButtonStyle::Transparent)
                            .tooltip(Tooltip::text(format!("Cancel this request ({CANCEL_KEYBINDING_HINT})")))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_request(cx);
                            })),
                    )
                })
                .when_some(elapsed_label, |this, label| {
                    this.child(
                        h_flex()
                            .gap_1()
                            .child(Indicator::dot().color(Color::Accent))
                            .child(
                                Label::new(label)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                }),
        );

        // ── Error display ─────────────────────────────────────────────────
        if let Some(err) = &self.error {
            content = content.child(
                Banner::new()
                    .severity(Severity::Error)
                    .children([Label::new(err.clone()).size(LabelSize::Small)]),
            );
        }

        // ── Response area ─────────────────────────────────────────────────
        if let Some(result) = &self.last_response {
            content = content.child(Divider::horizontal().color(DividerColor::Border));

            // Status · duration · size · copy row
            let size_label = format_file_size(result.body.len() as u64, true);
            content = content.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Chip::new(result.status.to_string())
                            .label_color(status_color(result.status))
                            .tooltip(Tooltip::text(http_status_text(result.status))),
                    )
                    .child(
                        // Wrap in an id'd div so we can attach a tooltip (Label doesn't support it).
                        div()
                            .id("duration-tip")
                            .tooltip(Tooltip::text("Time from request sent to response received"))
                            .child(
                                Label::new(format!("{}ms", result.duration_ms))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        Label::new(size_label)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(CopyButton::new("copy-resp-body", result.body.clone())),
            );

            // Response tab bar
            let active_resp_tab = self.active_response_tab;
            content = content.child(
                TabBar::new("response-tabs")
                    .child(
                        Tab::new("resp-tab-body")
                            .toggle_state(active_resp_tab == ResponseTab::Body)
                            .child(Label::new("Body").size(LabelSize::Small))
                            .tooltip(Tooltip::text("Response body"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.active_response_tab != ResponseTab::Body {
                                    this.active_response_tab = ResponseTab::Body;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Tab::new("resp-tab-headers")
                            .toggle_state(active_resp_tab == ResponseTab::Headers)
                            .child(Label::new("Headers").size(LabelSize::Small))
                            .tooltip(Tooltip::text("Response headers"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.active_response_tab != ResponseTab::Headers {
                                    this.active_response_tab = ResponseTab::Headers;
                                    cx.notify();
                                }
                            })),
                    ),
            );

            // Active response tab content
            match self.active_response_tab {
                ResponseTab::Body => {
                    content = content.child(
                        div()
                            .id("resp-body")
                            .p_2()
                            .rounded_md()
                            .bg(cx.theme().colors().editor_background)
                            .max_h(gpui::px(400.))
                            .overflow_y_scroll()
                            .child(self.response_editor.clone()),
                    );
                }
                ResponseTab::Headers => {
                    if result.headers.is_empty() {
                        content = content.child(
                            Label::new("No response headers")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        );
                    } else {
                        for (k, v) in &result.headers {
                            content = content.child(kv_row(k, v));
                        }
                    }
                }
            }
        }

        content
    }
}

/// Open a Postman request as a workspace tab, or activate an existing tab for the same request.
///
/// Dedup key is `(collection_name, request.path)` — same request in the same collection maps to
/// one tab no matter how many times the user clicks it.
pub(crate) fn open_postman_request(
    workspace: &mut Workspace,
    request: RequestItem,
    collection_id: String,
    collection_name: String,
    http_client: Arc<HttpClientWithUrl>,
    env_vars: Entity<HashMap<String, String>>,
    language_registry: Arc<LanguageRegistry>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    // Dedup by collection ID (not display name) + request path. Using the ID prevents
    // two collections with the same name from sharing tabs.
    let existing = workspace.active_pane().read(cx).items().find_map(|item| {
        let req_item = item.downcast::<PostmanRequestItem>()?;
        let matched = {
            let item = req_item.read(cx);
            item.collection_id == collection_id && item.request.path == request.path
        };
        if matched { Some(req_item) } else { None }
    });

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let item = cx.new(|cx: &mut Context<PostmanRequestItem>| {
            PostmanRequestItem::new(request, collection_id, collection_name, http_client, env_vars, language_registry, window, cx)
        });
        workspace.add_item_to_center(Box::new(item), window, cx);
    }
}

/// Register Postman request panel keybindings. Called once from `prism_hq::init`.
pub(crate) fn register_keybindings(cx: &mut gpui::App) {
    cx.bind_keys([
        KeyBinding::new("cmd-enter", SendPostmanRequest, Some("PostmanRequest")),
        KeyBinding::new("escape", CancelPostmanRequest, Some("PostmanRequest")),
        KeyBinding::new("cmd-l", FocusPostmanUrlBar, Some("PostmanRequest")),
    ]);
}

fn make_readonly_editor(
    initial_content: String,
    max_lines: usize,
    window: &mut Window,
    cx: &mut Context<PostmanRequestItem>,
) -> Entity<Editor> {
    cx.new(|cx| {
        let buffer = cx.new(|cx| Buffer::local(initial_content, cx));
        let multi_buffer = cx.new(|cx| MultiBuffer::singleton(buffer, cx));
        let mut editor = Editor::new(
            EditorMode::AutoHeight { min_lines: 1, max_lines: Some(max_lines) },
            multi_buffer,
            None,
            window,
            cx,
        );
        editor.set_read_only(true);
        editor.set_show_gutter(false, cx);
        editor
    })
}

fn http_status_text(status: u16) -> &'static str {
    match status {
        200 => "200 OK",
        201 => "201 Created",
        204 => "204 No Content",
        301 => "301 Moved Permanently",
        302 => "302 Found",
        304 => "304 Not Modified",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        422 => "422 Unprocessable Entity",
        429 => "429 Too Many Requests",
        500 => "500 Internal Server Error",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        504 => "504 Gateway Timeout",
        _ => "HTTP Response",
    }
}
