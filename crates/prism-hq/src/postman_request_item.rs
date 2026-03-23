use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use editor::{Editor, EditorMode};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Task, WeakEntity, Window,
};
use http_client::HttpClientWithUrl;
use language::{Buffer, LanguageRegistry};
use multi_buffer::MultiBuffer;
use project::project_settings::ProjectSettings;
use settings::Settings as _;
use ui::{
    Button, ButtonStyle, Chip, Color, CopyButton, Divider, DividerColor, Icon, IconName, Label,
    LabelSize, Tab, TabBar, h_flex, prelude::*, v_flex,
};
use ui_input::InputField;
use workspace::Workspace;
use workspace::item::{Item, ItemEvent, TabContentParams};

use crate::postman_panel::{
    ExecutionResult, PostmanHttpClient, RequestItem, RequestTab, ResponseTab, extract_url,
    kv_row, method_color, status_color, substitute_vars,
};

pub struct PostmanRequestItem {
    focus_handle: FocusHandle,
    /// Owns name, method, path, and request_def — avoids duplicating these fields.
    request: RequestItem,
    collection_name: String,
    http_client: Arc<HttpClientWithUrl>,
    env_vars: Arc<HashMap<String, String>>,
    active_request_tab: RequestTab,
    active_response_tab: ResponseTab,
    last_response: Option<ExecutionResult>,
    error: Option<String>,
    request_task: Option<Task<()>>,
    url_input: Entity<InputField>,
    response_editor: Entity<Editor>,
    language_registry: Arc<LanguageRegistry>,
}

impl PostmanRequestItem {
    pub(crate) fn new(
        request: RequestItem,
        collection_name: String,
        http_client: Arc<HttpClientWithUrl>,
        env_vars: Arc<HashMap<String, String>>,
        language_registry: Arc<LanguageRegistry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let resolved_url = extract_url(&request.request_def)
            .map(|u| substitute_vars(&u, &env_vars))
            .unwrap_or_default();

        let url_input = cx.new(|cx| {
            let field = InputField::new(window, cx, "https://…");
            field.set_text(&resolved_url, window, cx);
            field
        });

        let response_editor = cx.new(|cx| {
            let buffer = cx.new(|cx| Buffer::local("", cx));
            let multi_buffer = cx.new(|cx| MultiBuffer::singleton(buffer, cx));
            let mut editor = Editor::new(
                EditorMode::AutoHeight { min_lines: 1, max_lines: Some(50) },
                multi_buffer,
                None,
                window,
                cx,
            );
            editor.set_read_only(true);
            editor.set_show_gutter(false, cx);
            editor
        });

        Self {
            focus_handle,
            request,
            collection_name,
            http_client,
            env_vars,
            active_request_tab: RequestTab::Body,
            active_response_tab: ResponseTab::Body,
            last_response: None,
            error: None,
            request_task: None,
            url_input,
            response_editor,
            language_registry,
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
        let env_vars = self.env_vars.clone();
        self.last_response = None;
        self.error = None;
        cx.notify();

        self.request_task = Some(cx.spawn(async move |this: WeakEntity<PostmanRequestItem>, cx| {
            let result: Result<ExecutionResult> = cx
                .background_spawn(async move {
                    client.execute_request_with_url(&url, &request_def, &env_vars).await
                })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok(r) => {
                        // SharedString is Arc<str>-backed; clone avoids re-allocation.
                        let body = r.body.clone();
                        this.response_editor.read(cx).buffer().read(cx)
                            .as_singleton()
                            .unwrap()
                            .update(cx, |buf, cx| { buf.set_text(body, cx); });

                        // Async language assignment for JSON syntax highlighting.
                        let lang_reg = this.language_registry.clone();
                        let editor = this.response_editor.clone();
                        cx.spawn(async move |_this, cx| {
                            if let Ok(json_lang) = lang_reg.language_for_name("JSON").await {
                                cx.update(|cx| {
                                    editor.update(cx, |ed, cx| {
                                        ed.buffer().read(cx).as_singleton().unwrap()
                                            .update(cx, |buf, cx| buf.set_language(Some(json_lang), cx));
                                    });
                                });
                            }
                        }).detach();

                        this.last_response = Some(r);
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                this.request_task = None;
                cx.notify();
            })
            .ok();
        }));
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

        let mut content = v_flex()
            .id("postman-request-content")
            .key_context("PostmanRequest")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .overflow_y_scroll()
            .p_2()
            .gap_2();

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
                .child(Chip::new(self.request.method.clone()).label_color(method_clr))
                .child(self.url_input.clone()),
        );

        // ── Request tabs ─────────────────────────────────────────────────
        let active_req_tab = self.active_request_tab;
        content = content.child(
            TabBar::new("request-tabs")
                .child(
                    Tab::new("req-tab-params")
                        .toggle_state(active_req_tab == RequestTab::Params)
                        .child(Label::new("Params").size(LabelSize::Small))
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
                        Label::new("No query parameters")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    );
                } else {
                    for (k, v) in &all_params {
                        let v_resolved = substitute_vars(v, &self.env_vars);
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
                            let val_resolved = substitute_vars(val, &self.env_vars);
                            content = content.child(kv_row(key, &val_resolved));
                        }
                    }
                    _ => {
                        content = content.child(
                            Label::new("No headers")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        );
                    }
                }
            }
            RequestTab::Body => {
                let body_raw = self.request.request_def["body"]["raw"].as_str().unwrap_or("");
                if body_raw.is_empty() {
                    content = content.child(
                        Label::new("No request body")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    );
                } else {
                    let body_resolved = substitute_vars(body_raw, &self.env_vars);
                    content = content.child(
                        div()
                            .p_2()
                            .bg(cx.theme().colors().editor_background)
                            .rounded_md()
                            .child(Label::new(body_resolved).size(LabelSize::Small)),
                    );
                }
            }
        }

        // ── Send button ───────────────────────────────────────────────────
        let is_sending = self.request_task.is_some();
        let send_label = if is_sending { "Sending…" } else { "▶ Send" };
        content = content.child(
            Button::new("send-request", send_label)
                .style(ButtonStyle::Filled)
                .disabled(is_sending)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.run_request(cx);
                })),
        );

        // ── Error display ─────────────────────────────────────────────────
        if let Some(err) = &self.error {
            content = content.child(
                Label::new(format!("Error: {err}"))
                    .size(LabelSize::Small)
                    .color(Color::Error),
            );
        }

        // ── Response area ─────────────────────────────────────────────────
        if let Some(result) = &self.last_response {
            content = content.child(Divider::horizontal().color(DividerColor::Border));

            // Status + duration + copy row
            content = content.child(
                h_flex()
                    .gap_2()
                    .child(
                        Chip::new(result.status.to_string()).label_color(status_color(result.status)),
                    )
                    .child(
                        Label::new(format!("{}ms", result.duration_ms))
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
    collection_name: String,
    http_client: Arc<HttpClientWithUrl>,
    env_vars: Arc<HashMap<String, String>>,
    language_registry: Arc<LanguageRegistry>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let existing = workspace.active_pane().read(cx).items().find_map(|item| {
        let req_item = item.downcast::<PostmanRequestItem>()?;
        let matched = {
            let item = req_item.read(cx);
            item.collection_name == collection_name && item.request.path == request.path
        };
        if matched { Some(req_item) } else { None }
    });

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let col_name = collection_name.clone();
        let item = cx.new(|cx: &mut Context<PostmanRequestItem>| {
            PostmanRequestItem::new(request, col_name, http_client, env_vars, language_registry, window, cx)
        });
        workspace.add_item_to_center(Box::new(item), window, cx);
    }
}
