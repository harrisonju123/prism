use anyhow::{Context as _, Result};
use futures::{AsyncReadExt, FutureExt, StreamExt, future::BoxFuture};
use gpui::{AnyView, App, AsyncApp, Context, Entity, SharedString, Task, Window};
use http_client::{AsyncBody, HttpClient, Method, Request as HttpRequest};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use language_model::{
    ApiKeyState, AuthenticateError, EnvVar, IconOrSvg, LanguageModel, LanguageModelCompletionError,
    LanguageModelCompletionEvent, LanguageModelId, LanguageModelName, LanguageModelProvider,
    LanguageModelProviderId, LanguageModelProviderName, LanguageModelProviderState,
    LanguageModelRequest, LanguageModelToolChoice, LanguageModelToolSchemaFormat, RateLimiter,
    env_var,
};
use open_ai::{
    ResponseStreamEvent,
    responses::{Request as ResponseRequest, StreamEvent as ResponsesStreamEvent, stream_response},
    stream_completion,
};
use settings::{Settings, SettingsStore};
use std::sync::{Arc, LazyLock};
use ui::{ElevationIndex, Tooltip, prelude::*};
use ui_input::InputField;
use util::ResultExt;

use crate::provider::open_ai::{
    OpenAiEventMapper, OpenAiResponseEventMapper, into_open_ai, into_open_ai_response,
};
pub use settings::OpenAiCompatibleModelCapabilities as ModelCapabilities;
pub use settings::PrismAvailableModel as AvailableModel;

const PROVIDER_ID: LanguageModelProviderId = LanguageModelProviderId::new("prism");
const PROVIDER_NAME: LanguageModelProviderName = LanguageModelProviderName::new("PrisM");

const API_KEY_ENV_VAR_NAME: &str = "PRISM_API_KEY";
static API_KEY_ENV_VAR: LazyLock<EnvVar> = env_var!(API_KEY_ENV_VAR_NAME);

#[derive(Default, Clone, Debug, PartialEq)]
pub struct PrismSettings {
    pub api_url: String,
    pub available_models: Vec<AvailableModel>,
}

pub struct PrismLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    state: Entity<State>,
}

#[derive(Debug, Clone, PartialEq)]
enum SidecarStatus {
    Unknown,
    External,
    Sidecar,
    NotFound,
}

pub struct State {
    api_key_state: ApiKeyState,
    settings: PrismSettings,
    http_client: Arc<dyn HttpClient>,
    fetched_models: Vec<String>,
    fetch_models_task: Option<Task<Result<()>>>,
    sidecar: Option<smol::process::Child>,
    sidecar_status: SidecarStatus,
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(mut child) = self.sidecar.take() {
            if let Err(err) = child.kill() {
                log::warn!("Failed to kill PrisM sidecar process: {err}");
            }
        }
    }
}

impl State {
    fn is_authenticated(&self) -> bool {
        self.api_key_state.has_key()
    }

    fn set_api_key(&mut self, api_key: Option<String>, cx: &mut Context<Self>) -> Task<Result<()>> {
        let api_url = SharedString::new(self.settings.api_url.as_str());
        self.api_key_state
            .store(api_url, api_key, |this| &mut this.api_key_state, cx)
    }

    fn authenticate(&mut self, cx: &mut Context<Self>) -> Task<Result<(), AuthenticateError>> {
        let api_url = SharedString::new(self.settings.api_url.clone());
        let auth_task = self
            .api_key_state
            .load_if_needed(api_url, |this| &mut this.api_key_state, cx);
        cx.spawn(async move |this, cx| {
            let result = auth_task.await;
            if result.is_ok() {
                this.update(cx, |this, cx| this.restart_fetch_models_task(cx))
                    .ok();
            }
            result
        })
    }

    fn fetch_models(&mut self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let http_client = self.http_client.clone();
        let api_url = self.settings.api_url.clone();
        let api_key = self.api_key_state.key(&api_url);
        let sidecar_already_running = self.sidecar.is_some();

        cx.spawn(async move |this, cx| {
            let Some(api_key) = api_key else {
                return Ok(());
            };

            match do_fetch_models(&http_client, &api_url, &api_key).await {
                Ok(models) => {
                    this.update(cx, |this, cx| {
                        if this.sidecar.is_none() {
                            this.sidecar_status = SidecarStatus::External;
                        }
                        this.fetched_models = models;
                        cx.notify();
                    })
                }
                Err(err) if is_connection_refused(&err) => {
                    if !sidecar_already_running {
                        let port = port_from_url(&api_url);
                        match find_prism_binary() {
                            Some(path) => match spawn_prism_sidecar(&path, &port) {
                                Ok(child) => {
                                    this.update(cx, |this, cx| {
                                        this.sidecar = Some(child);
                                        this.sidecar_status = SidecarStatus::Sidecar;
                                        cx.notify();
                                    })
                                    .ok();
                                }
                                Err(spawn_err) => {
                                    log::warn!("Failed to spawn PrisM sidecar: {spawn_err}");
                                    this.update(cx, |this, cx| {
                                        this.sidecar_status = SidecarStatus::NotFound;
                                        cx.notify();
                                    })
                                    .ok();
                                    return Ok(());
                                }
                            },
                            None => {
                                this.update(cx, |this, cx| {
                                    this.sidecar_status = SidecarStatus::NotFound;
                                    cx.notify();
                                })
                                .ok();
                                return Ok(());
                            }
                        }
                    }

                    // Retry with backoff while the sidecar starts up (up to 5 × 1500ms = 7.5s).
                    for _ in 0..5 {
                        smol::Timer::after(Duration::from_millis(1500)).await;
                        match do_fetch_models(&http_client, &api_url, &api_key).await {
                            Ok(models) => {
                                return this.update(cx, |this, cx| {
                                    this.fetched_models = models;
                                    cx.notify();
                                });
                            }
                            Err(retry_err) if is_connection_refused(&retry_err) => continue,
                            Err(retry_err) => return Err(retry_err),
                        }
                    }

                    Ok(())
                }
                Err(err) => Err(err),
            }
        })
    }

    fn restart_fetch_models_task(&mut self, cx: &mut Context<Self>) {
        let task = self.fetch_models(cx);
        self.fetch_models_task.replace(task);
    }
}

impl PrismLanguageModelProvider {
    pub fn new(http_client: Arc<dyn HttpClient>, cx: &mut App) -> Self {
        let state = cx.new({
            let http_client = http_client.clone();
            move |cx| {
                cx.observe_global::<SettingsStore>(|this: &mut State, cx| {
                    let settings = crate::AllLanguageModelSettings::get_global(cx)
                        .prism
                        .clone();
                    if this.settings != settings {
                        let api_url = SharedString::new(settings.api_url.as_str());
                        this.api_key_state.handle_url_change(
                            api_url,
                            |this| &mut this.api_key_state,
                            cx,
                        );
                        if this.settings.api_url != settings.api_url && this.is_authenticated() {
                            this.restart_fetch_models_task(cx);
                        }
                        this.settings = settings;
                        cx.notify();
                    }
                })
                .detach();
                let settings = crate::AllLanguageModelSettings::get_global(cx)
                    .prism
                    .clone();
                State {
                    api_key_state: ApiKeyState::new(
                        SharedString::new(settings.api_url.as_str()),
                        API_KEY_ENV_VAR.clone(),
                    ),
                    settings,
                    http_client,
                    fetched_models: Vec::new(),
                    fetch_models_task: None,
                    sidecar: None,
                    sidecar_status: SidecarStatus::Unknown,
                }
            }
        });

        Self {
            http_client,
            state,
        }
    }

    fn create_language_model(&self, model: AvailableModel) -> Arc<dyn LanguageModel> {
        Arc::new(PrismLanguageModel {
            id: LanguageModelId::from(model.name.clone()),
            provider_id: PROVIDER_ID,
            provider_name: PROVIDER_NAME,
            model,
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }
}

impl LanguageModelProviderState for PrismLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

impl LanguageModelProvider for PrismLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        PROVIDER_ID
    }

    fn name(&self) -> LanguageModelProviderName {
        PROVIDER_NAME
    }

    fn icon(&self) -> IconOrSvg {
        IconOrSvg::Icon(IconName::AiOpenAiCompat)
    }

    fn default_model(&self, cx: &App) -> Option<Arc<dyn LanguageModel>> {
        self.provided_models(cx).into_iter().next()
    }

    fn default_fast_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        None
    }

    fn provided_models(&self, cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        let state = self.state.read(cx);
        let mut models: HashMap<String, AvailableModel> = state
            .fetched_models
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    AvailableModel {
                        name: id.clone(),
                        display_name: None,
                        max_tokens: 128_000,
                        max_output_tokens: None,
                        capabilities: Default::default(),
                    },
                )
            })
            .collect();
        for model in &state.settings.available_models {
            models.insert(model.name.clone(), model.clone());
        }
        let mut result: Vec<Arc<dyn LanguageModel>> = models
            .into_values()
            .map(|m| self.create_language_model(m))
            .collect();
        result.sort_by_key(|m| m.name());
        result
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        self.state.update(cx, |state, cx| state.authenticate(cx))
    }

    fn configuration_view(
        &self,
        _target_agent: language_model::ConfigurationViewTargetAgent,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyView {
        cx.new(|cx| ConfigurationView::new(self.state.clone(), window, cx))
            .into()
    }

    fn reset_credentials(&self, cx: &mut App) -> Task<Result<()>> {
        self.state
            .update(cx, |state, cx| state.set_api_key(None, cx))
    }
}

pub struct PrismLanguageModel {
    id: LanguageModelId,
    provider_id: LanguageModelProviderId,
    provider_name: LanguageModelProviderName,
    model: AvailableModel,
    state: Entity<State>,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
}

impl PrismLanguageModel {
    fn stream_completion(
        &self,
        request: open_ai::Request,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<
            futures::stream::BoxStream<'static, Result<ResponseStreamEvent>>,
            LanguageModelCompletionError,
        >,
    > {
        let http_client = self.http_client.clone();

        let (api_key, api_url) = self.state.read_with(cx, |state, _cx| {
            let api_url = &state.settings.api_url;
            (
                state.api_key_state.key(api_url),
                state.settings.api_url.clone(),
            )
        });

        let provider = self.provider_name.clone();
        let future = self.request_limiter.stream(async move {
            let Some(api_key) = api_key else {
                return Err(LanguageModelCompletionError::NoApiKey { provider });
            };
            let response = stream_completion(
                http_client.as_ref(),
                provider.0.as_str(),
                &api_url,
                &api_key,
                request,
            )
            .await?;
            Ok(response)
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }

    fn stream_response(
        &self,
        request: ResponseRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<futures::stream::BoxStream<'static, Result<ResponsesStreamEvent>>>>
    {
        let http_client = self.http_client.clone();

        let (api_key, api_url) = self.state.read_with(cx, |state, _cx| {
            let api_url = &state.settings.api_url;
            (
                state.api_key_state.key(api_url),
                state.settings.api_url.clone(),
            )
        });

        let provider = self.provider_name.clone();
        let future = self.request_limiter.stream(async move {
            let Some(api_key) = api_key else {
                return Err(LanguageModelCompletionError::NoApiKey { provider });
            };
            let response = stream_response(
                http_client.as_ref(),
                provider.0.as_str(),
                &api_url,
                &api_key,
                request,
            )
            .await?;
            Ok(response)
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }
}

impl LanguageModel for PrismLanguageModel {
    fn id(&self) -> LanguageModelId {
        self.id.clone()
    }

    fn name(&self) -> LanguageModelName {
        LanguageModelName::from(
            self.model
                .display_name
                .clone()
                .unwrap_or_else(|| self.model.name.clone()),
        )
    }

    fn provider_id(&self) -> LanguageModelProviderId {
        self.provider_id.clone()
    }

    fn provider_name(&self) -> LanguageModelProviderName {
        self.provider_name.clone()
    }

    fn supports_tools(&self) -> bool {
        self.model.capabilities.tools
    }

    fn tool_input_format(&self) -> LanguageModelToolSchemaFormat {
        LanguageModelToolSchemaFormat::JsonSchemaSubset
    }

    fn supports_images(&self) -> bool {
        self.model.capabilities.images
    }

    fn supports_tool_choice(&self, choice: LanguageModelToolChoice) -> bool {
        match choice {
            LanguageModelToolChoice::Auto => self.model.capabilities.tools,
            LanguageModelToolChoice::Any => self.model.capabilities.tools,
            LanguageModelToolChoice::None => true,
        }
    }

    fn supports_streaming_tools(&self) -> bool {
        true
    }

    fn supports_split_token_display(&self) -> bool {
        true
    }

    fn telemetry_id(&self) -> String {
        format!("prism/{}", self.model.name)
    }

    fn max_token_count(&self) -> u64 {
        self.model.max_tokens
    }

    fn max_output_tokens(&self) -> Option<u64> {
        self.model.max_output_tokens
    }

    fn count_tokens(
        &self,
        request: LanguageModelRequest,
        cx: &App,
    ) -> BoxFuture<'static, Result<u64>> {
        let max_token_count = self.max_token_count();
        cx.background_spawn(async move {
            let messages = super::open_ai::collect_tiktoken_messages(request);
            let model = if max_token_count >= 100_000 {
                "gpt-4o"
            } else {
                "gpt-4"
            };
            tiktoken_rs::num_tokens_from_messages(model, &messages).map(|tokens| tokens as u64)
        })
        .boxed()
    }

    fn stream_completion(
        &self,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<
            futures::stream::BoxStream<
                'static,
                Result<LanguageModelCompletionEvent, LanguageModelCompletionError>,
            >,
            LanguageModelCompletionError,
        >,
    > {
        if self.model.capabilities.chat_completions {
            let request = into_open_ai(
                request,
                &self.model.name,
                self.model.capabilities.parallel_tool_calls,
                self.model.capabilities.prompt_cache_key,
                self.max_output_tokens(),
                None,
            );
            let completions = self.stream_completion(request, cx);
            async move {
                let mapper = OpenAiEventMapper::new();
                Ok(mapper.map_stream(completions.await?).boxed())
            }
            .boxed()
        } else {
            let request = into_open_ai_response(
                request,
                &self.model.name,
                self.model.capabilities.parallel_tool_calls,
                self.model.capabilities.prompt_cache_key,
                self.max_output_tokens(),
                None,
            );
            let completions = self.stream_response(request, cx);
            async move {
                let mapper = OpenAiResponseEventMapper::new();
                Ok(mapper.map_stream(completions.await?).boxed())
            }
            .boxed()
        }
    }
}

struct ConfigurationView {
    api_key_editor: Entity<InputField>,
    state: Entity<State>,
    load_credentials_task: Option<Task<()>>,
}

impl ConfigurationView {
    fn new(state: Entity<State>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let api_key_editor = cx.new(|cx| {
            InputField::new(
                window,
                cx,
                "000000000000000000000000000000000000000000000000000",
            )
        });

        cx.observe(&state, |_, _, cx| {
            cx.notify();
        })
        .detach();

        let load_credentials_task = Some(cx.spawn_in(window, {
            let state = state.clone();
            async move |this, cx| {
                if let Some(task) = Some(state.update(cx, |state, cx| state.authenticate(cx))) {
                    let _ = task.await;
                }
                this.update(cx, |this, cx| {
                    this.load_credentials_task = None;
                    cx.notify();
                })
                .log_err();
            }
        }));

        Self {
            api_key_editor,
            state,
            load_credentials_task,
        }
    }

    fn save_api_key(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let api_key = self.api_key_editor.read(cx).text(cx).trim().to_string();
        if api_key.is_empty() {
            return;
        }

        self.api_key_editor
            .update(cx, |input, cx| input.set_text("", window, cx));

        let state = self.state.clone();
        cx.spawn_in(window, async move |_, cx| {
            state
                .update(cx, |state, cx| state.set_api_key(Some(api_key), cx))
                .await
        })
        .detach_and_log_err(cx);
    }

    fn reset_api_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.api_key_editor
            .update(cx, |input, cx| input.set_text("", window, cx));

        let state = self.state.clone();
        cx.spawn_in(window, async move |_, cx| {
            state
                .update(cx, |state, cx| state.set_api_key(None, cx))
                .await
        })
        .detach_and_log_err(cx);
    }

    fn should_render_editor(&self, cx: &Context<Self>) -> bool {
        !self.state.read(cx).is_authenticated()
    }
}

impl Render for ConfigurationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let env_var_set = state.api_key_state.is_from_env_var();
        let env_var_name = state.api_key_state.env_var_name();
        let sidecar_status = state.sidecar_status.clone();

        let api_key_section = if self.should_render_editor(cx) {
            v_flex()
                .on_action(cx.listener(Self::save_api_key))
                .child(Label::new(
                    "To use Zed's agent with PrisM, enter your PrisM API key.",
                ))
                .child(
                    div()
                        .pt(DynamicSpacing::Base04.rems(cx))
                        .child(self.api_key_editor.clone()),
                )
                .child(
                    Label::new(format!(
                        "You can also set the {env_var_name} environment variable and restart Zed."
                    ))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any()
        } else {
            h_flex()
                .mt_1()
                .p_1()
                .justify_between()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().colors().border)
                .bg(cx.theme().colors().background)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(Icon::new(IconName::Check).color(Color::Success))
                        .child(
                            div()
                                .w_full()
                                .overflow_x_hidden()
                                .text_ellipsis()
                                .child(Label::new(if env_var_set {
                                    format!("API key set in {env_var_name} environment variable")
                                } else {
                                    format!(
                                        "API key configured for {}",
                                        &state.settings.api_url
                                    )
                                })),
                        ),
                )
                .child(
                    h_flex().flex_shrink_0().child(
                        Button::new("reset-api-key", "Reset API Key")
                            .label_size(LabelSize::Small)
                            .icon(IconName::Undo)
                            .icon_size(IconSize::Small)
                            .icon_position(IconPosition::Start)
                            .layer(ElevationIndex::ModalSurface)
                            .when(env_var_set, |this| {
                                this.tooltip(Tooltip::text(format!(
                                    "To reset your API key, unset the {env_var_name} environment variable."
                                )))
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.reset_api_key(window, cx)),
                            ),
                    ),
                )
                .into_any()
        };

        let sidecar_status_label = match sidecar_status {
            SidecarStatus::Sidecar => Some(
                Label::new("PrisM running (sidecar)")
                    .size(LabelSize::Small)
                    .color(Color::Success),
            ),
            SidecarStatus::External => Some(
                Label::new("PrisM running (external)")
                    .size(LabelSize::Small)
                    .color(Color::Success),
            ),
            SidecarStatus::NotFound => Some(
                Label::new("PrisM binary not found — install with: cargo install prism")
                    .size(LabelSize::Small)
                    .color(Color::Warning),
            ),
            SidecarStatus::Unknown => None,
        };

        if self.load_credentials_task.is_some() {
            div().child(Label::new("Loading credentials…")).into_any()
        } else {
            v_flex()
                .size_full()
                .child(api_key_section)
                .when_some(sidecar_status_label, |this, label| {
                    this.child(div().pt(DynamicSpacing::Base04.rems(cx)).child(label))
                })
                .into_any()
        }
    }
}

fn is_connection_refused(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            if io_err.kind() == std::io::ErrorKind::ConnectionRefused {
                return true;
            }
        }
    }
    false
}

fn port_from_url(api_url: &str) -> String {
    // Parse port from URLs like "http://localhost:9100/v1".
    api_url
        .split(':')
        .nth(2)
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(9100)
        .to_string()
}

fn find_prism_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PRISM_BIN") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in &["prism", "prism-gateway"] {
                let candidate = dir.join(name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let candidate = PathBuf::from(home).join(".cargo/bin/prism");
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

fn spawn_prism_sidecar(
    path: &PathBuf,
    port: &str,
) -> Result<smol::process::Child> {
    smol::process::Command::new(path)
        .arg("--port")
        .arg(port)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Failed to spawn PrisM sidecar process")
}

async fn do_fetch_models(
    http_client: &Arc<dyn HttpClient>,
    api_url: &str,
    api_key: &str,
) -> Result<Vec<String>> {
    let uri = format!("{api_url}/models");
    let request = HttpRequest::builder()
        .method(Method::GET)
        .uri(uri)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .body(AsyncBody::default())?;

    let mut response = http_client.send(request).await?;
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;

    anyhow::ensure!(
        response.status().is_success(),
        "PrisM /v1/models failed: {} {}",
        response.status(),
        body
    );

    let parsed: PrismModelsResponse =
        serde_json::from_str(&body).context("failed to parse PrisM models response")?;

    Ok(parsed.data.into_iter().map(|m| m.id).collect())
}

#[derive(serde::Deserialize)]
struct PrismModelsResponse {
    data: Vec<PrismModelEntry>,
}

#[derive(serde::Deserialize)]
struct PrismModelEntry {
    id: String,
}
