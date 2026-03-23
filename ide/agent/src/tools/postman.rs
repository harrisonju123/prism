use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use agent_client_protocol as acp;
use anyhow::{Context as _, Result, bail};
use futures::{AsyncReadExt as _, FutureExt as _};
use gpui::{App, AppContext as _, Task};
use http_client::{AsyncBody, HttpClientWithUrl, Method};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ui::SharedString;

use crate::{
    AgentTool, ToolCallEventStream, ToolInput, ToolPermissionDecision,
    decide_permission_from_settings,
};

// ── Shared types ──────────────────────────────────────────────────────────

struct RequestResult {
    url: String,
    status: u16,
    duration_ms: u128,
    headers: Vec<(String, String)>,
    /// Pretty-printed response body, truncated to 4KB.
    body: String,
}

// ── PostmanClient ──────────────────────────────────────────────────────────

pub struct PostmanClient {
    http_client: Arc<HttpClientWithUrl>,
    api_key: String,
}

impl PostmanClient {
    pub fn new(http_client: Arc<HttpClientWithUrl>, api_key: String) -> Self {
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

    async fn list_collections(&self) -> Result<serde_json::Value> {
        self.get_json("/collections").await
    }

    async fn get_collection(&self, id: &str) -> Result<serde_json::Value> {
        self.get_json(&format!("/collections/{id}")).await
    }

    async fn list_environments(&self) -> Result<serde_json::Value> {
        self.get_json("/environments").await
    }

    async fn get_environment(&self, id: &str) -> Result<serde_json::Value> {
        self.get_json(&format!("/environments/{id}")).await
    }

    /// Execute an HTTP request defined by the Postman request object, substituting
    /// `{{variable}}` placeholders with values from `env_vars`.
    async fn execute_request(
        &self,
        request_def: &serde_json::Value,
        env_vars: &HashMap<String, String>,
    ) -> Result<String> {
        let result = self
            .execute_request_detailed(request_def, env_vars, None)
            .await?;
        let headers_map: serde_json::Map<String, serde_json::Value> = result
            .headers
            .into_iter()
            .map(|(k, v)| (k, serde_json::Value::String(v)))
            .collect();
        Ok(format!(
            "Status: {status}\nDuration: {duration_ms}ms\nHeaders: {headers_json}\n\nBody:\n{body}",
            status = result.status,
            duration_ms = result.duration_ms,
            headers_json = serde_json::to_string(&headers_map).unwrap_or_default(),
            body = result.body,
        ))
    }

    /// Like `execute_request` but returns structured data. When `base_url_override`
    /// is set, the scheme+host+port of the resolved URL is replaced with that value
    /// before sending — used by `postman_test_endpoint` to redirect traffic to the
    /// local dev server without mutating the Postman collection.
    async fn execute_request_detailed(
        &self,
        request_def: &serde_json::Value,
        env_vars: &HashMap<String, String>,
        base_url_override: Option<&str>,
    ) -> Result<RequestResult> {
        let method_str = request_def["method"]
            .as_str()
            .unwrap_or("GET")
            .to_uppercase();

        let url_raw = extract_url(request_def).context("could not determine request URL")?;
        let url_substituted = substitute_vars(&url_raw, env_vars);

        // Replace origin when overriding — keeps path/query/fragment intact.
        let url = if let Some(base) = base_url_override {
            replace_origin(&url_substituted, base)
        } else {
            url_substituted
        };

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

        let start = Instant::now();
        let mut response = self
            .http_client
            .send(request)
            .await
            .with_context(|| format!("request to {url} failed"))?;
        let duration_ms = start.elapsed().as_millis();

        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
            .collect();

        let mut resp_body_bytes = Vec::new();
        response
            .body_mut()
            .read_to_end(&mut resp_body_bytes)
            .await
            .context("error reading response body")?;
        let body_text = String::from_utf8_lossy(&resp_body_bytes).into_owned();

        let mut body = serde_json::from_str::<serde_json::Value>(&body_text)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or(body_text);
        body.truncate(4096);

        Ok(RequestResult {
            url,
            status,
            duration_ms,
            headers,
            body,
        })
    }
}

fn extract_url(request_def: &serde_json::Value) -> Option<String> {
    let url = &request_def["url"];
    // String form, or object with raw field (the common case)
    if let Some(s) = url.as_str().or_else(|| url["raw"].as_str()) {
        return Some(s.to_string());
    }
    // Fallback: reconstruct from decomposed parts (protocol/host/path/query).
    // Postman always writes `raw` alongside the parts, but older or programmatically
    // generated collections may omit it.
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
            let subst = substitute_vars(raw, vars);
            AsyncBody::from(subst)
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

fn json_pretty(v: &serde_json::Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

// ── Tool: postman_list_collections ────────────────────────────────────────

/// List all Postman collections available in your workspace.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostmanListCollectionsInput {}

pub struct PostmanListCollectionsTool {
    client: Arc<PostmanClient>,
}

impl PostmanListCollectionsTool {
    pub fn new(client: Arc<PostmanClient>) -> Self {
        Self { client }
    }
}

impl AgentTool for PostmanListCollectionsTool {
    type Input = PostmanListCollectionsInput;
    type Output = String;

    const NAME: &'static str = "postman_list_collections";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "List Postman Collections".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        let client = self.client.clone();
        cx.spawn(async move |_cx| {
            input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive input: {e}"))?;

            let decision =
                decide_permission_from_settings(Self::NAME, &[], event_stream.tool_permissions());
            if let ToolPermissionDecision::Deny(reason) = decision {
                return Err(reason);
            }

            let json = client.list_collections().await.map_err(|e| e.to_string())?;
            Ok(json_pretty(&json))
        })
    }
}

// ── Tool: postman_get_collection ──────────────────────────────────────────

/// Retrieve the full structure of a Postman collection, including all folders,
/// requests, methods, and URLs.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostmanGetCollectionInput {
    /// The ID of the collection to retrieve.
    collection_id: String,
}

pub struct PostmanGetCollectionTool {
    client: Arc<PostmanClient>,
}

impl PostmanGetCollectionTool {
    pub fn new(client: Arc<PostmanClient>) -> Self {
        Self { client }
    }
}

impl AgentTool for PostmanGetCollectionTool {
    type Input = PostmanGetCollectionInput;
    type Output = String;

    const NAME: &'static str = "postman_get_collection";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        match input {
            Ok(i) => format!("Get Postman Collection: {}", i.collection_id).into(),
            Err(_) => "Get Postman Collection".into(),
        }
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        let client = self.client.clone();
        cx.spawn(async move |_cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive input: {e}"))?;

            let decision = decide_permission_from_settings(
                Self::NAME,
                std::slice::from_ref(&input.collection_id),
                event_stream.tool_permissions(),
            );
            if let ToolPermissionDecision::Deny(reason) = decision {
                return Err(reason);
            }

            let json = client
                .get_collection(&input.collection_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json_pretty(&json))
        })
    }
}

// ── Tool: postman_list_environments ───────────────────────────────────────

/// List all Postman environments available in your workspace.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostmanListEnvironmentsInput {}

pub struct PostmanListEnvironmentsTool {
    client: Arc<PostmanClient>,
}

impl PostmanListEnvironmentsTool {
    pub fn new(client: Arc<PostmanClient>) -> Self {
        Self { client }
    }
}

impl AgentTool for PostmanListEnvironmentsTool {
    type Input = PostmanListEnvironmentsInput;
    type Output = String;

    const NAME: &'static str = "postman_list_environments";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "List Postman Environments".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        let client = self.client.clone();
        cx.spawn(async move |_cx| {
            input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive input: {e}"))?;

            let decision =
                decide_permission_from_settings(Self::NAME, &[], event_stream.tool_permissions());
            if let ToolPermissionDecision::Deny(reason) = decision {
                return Err(reason);
            }

            let json = client
                .list_environments()
                .await
                .map_err(|e| e.to_string())?;
            Ok(json_pretty(&json))
        })
    }
}

// ── Tool: postman_get_environment ─────────────────────────────────────────

/// Retrieve the variables (key/value/enabled) for a Postman environment.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostmanGetEnvironmentInput {
    /// The ID of the environment to retrieve.
    environment_id: String,
}

pub struct PostmanGetEnvironmentTool {
    client: Arc<PostmanClient>,
}

impl PostmanGetEnvironmentTool {
    pub fn new(client: Arc<PostmanClient>) -> Self {
        Self { client }
    }
}

impl AgentTool for PostmanGetEnvironmentTool {
    type Input = PostmanGetEnvironmentInput;
    type Output = String;

    const NAME: &'static str = "postman_get_environment";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        match input {
            Ok(i) => format!("Get Postman Environment: {}", i.environment_id).into(),
            Err(_) => "Get Postman Environment".into(),
        }
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        let client = self.client.clone();
        cx.spawn(async move |_cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive input: {e}"))?;

            let decision = decide_permission_from_settings(
                Self::NAME,
                std::slice::from_ref(&input.environment_id),
                event_stream.tool_permissions(),
            );
            if let ToolPermissionDecision::Deny(reason) = decision {
                return Err(reason);
            }

            let json = client
                .get_environment(&input.environment_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json_pretty(&json))
        })
    }
}

// ── Tool: postman_run_request ─────────────────────────────────────────────

/// Execute a specific request from a Postman collection. Resolves `{{variable}}`
/// placeholders using the specified environment (if provided).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostmanRunRequestInput {
    /// The ID of the collection containing the request.
    collection_id: String,
    /// Path to the request within the collection, e.g. "Folder/Request Name".
    request_path: String,
    /// Optional environment ID to resolve variables.
    environment_id: Option<String>,
}

pub struct PostmanRunRequestTool {
    client: Arc<PostmanClient>,
}

impl PostmanRunRequestTool {
    pub fn new(client: Arc<PostmanClient>) -> Self {
        Self { client }
    }
}

impl AgentTool for PostmanRunRequestTool {
    type Input = PostmanRunRequestInput;
    type Output = String;

    const NAME: &'static str = "postman_run_request";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Fetch
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        match input {
            Ok(i) => format!("Run Postman request: {}", i.request_path).into(),
            Err(_) => "Run Postman request".into(),
        }
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        let client = self.client.clone();
        cx.spawn(async move |cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive input: {e}"))?;

            // Fetch-kind tools always require explicit user authorization.
            let context = crate::ToolPermissionContext::new(
                Self::NAME,
                vec![format!("{} / {}", input.collection_id, input.request_path)],
            );
            let authorize = cx.update(|cx| {
                event_stream.authorize(
                    format!("Run Postman request: {}", input.request_path),
                    context,
                    cx,
                )
            });
            authorize.await.map_err(|e| e.to_string())?;

            let run_task = cx.background_spawn(async move {
                let collection_json = client.get_collection(&input.collection_id).await?;
                let request_def = find_request(&collection_json, &input.request_path)
                    .with_context(|| {
                        format!("request '{}' not found in collection", input.request_path)
                    })?;

                let env_vars = if let Some(env_id) = &input.environment_id {
                    let env_json = client.get_environment(env_id).await?;
                    parse_env_vars(&env_json)
                } else {
                    HashMap::new()
                };

                client.execute_request(request_def, &env_vars).await
            });

            futures::select! {
                result = run_task.fuse() => result.map_err(|e| e.to_string()),
                _ = event_stream.cancelled_by_user().fuse() => {
                    Err("Request cancelled by user".to_string())
                }
            }
        })
    }
}

/// Walk the collection item tree to find a request by path like "Folder/Request Name".
fn find_request<'a>(
    collection: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let (name, rest) = match path.split_once('/') {
        Some((n, r)) => (n.trim(), Some(r.trim())),
        None => (path.trim(), None),
    };

    let items = collection["collection"]["item"]
        .as_array()
        .or_else(|| collection["item"].as_array())?;

    for item in items {
        let item_name = item["name"].as_str().unwrap_or("");
        if item_name == name {
            if let Some(rest_path) = rest {
                // Recurse into folder
                return find_request(item, rest_path);
            } else if item["request"].is_object() {
                return Some(&item["request"]);
            }
        }
    }
    None
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

/// Returns the byte index of the first `/` that starts the path (after `://` + host).
/// Returns `url.len()` if no path is found (bare host with no trailing slash).
fn url_path_start(url: &str) -> usize {
    url.find("://")
        .and_then(|i| url[i + 3..].find('/').map(|j| i + 3 + j))
        .unwrap_or(url.len())
}

/// Replace the scheme+host+port of `url` with `base`, preserving path/query/fragment.
///
/// e.g. replace_origin("https://api.example.com/v1/users?a=1", "http://localhost:8080")
///      → "http://localhost:8080/v1/users?a=1"
fn replace_origin(url: &str, base: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}{}", &url[url_path_start(url)..])
}

struct MatchedRequest<'a> {
    name: String,
    folder_path: String,
    request_def: &'a serde_json::Value,
}

/// Walk the collection item tree looking for requests whose method and URL path
/// match the given criteria. Method match is case-insensitive; path match checks
/// whether the request URL path contains `path_pattern` (lowercased).
fn find_requests_by_endpoint<'a>(
    collection: &'a serde_json::Value,
    method: &str,
    path_pattern: &str,
) -> Vec<MatchedRequest<'a>> {
    let mut results = Vec::new();
    let items = match collection["collection"]["item"]
        .as_array()
        .or_else(|| collection["item"].as_array())
    {
        Some(a) => a,
        None => return results,
    };
    // Normalize once here; walk_items passes them through unchanged on recursion.
    let method_upper = method.to_uppercase();
    let pattern_lower = path_pattern.to_lowercase();
    walk_items(items, &method_upper, &pattern_lower, "", &mut results);
    results
}

fn walk_items<'a>(
    items: &'a [serde_json::Value],
    method_upper: &str,
    pattern_lower: &str,
    folder: &str,
    out: &mut Vec<MatchedRequest<'a>>,
) {
    for item in items {
        if let Some(sub_items) = item["item"].as_array() {
            // Folder — recurse
            let name = item["name"].as_str().unwrap_or("");
            let child_folder = if folder.is_empty() {
                name.to_string()
            } else {
                format!("{folder}/{name}")
            };
            walk_items(sub_items, method_upper, pattern_lower, &child_folder, out);
        } else if item["request"].is_object() {
            let req_val = &item["request"];
            let req_method = req_val["method"].as_str().unwrap_or("").to_uppercase();
            if req_method != method_upper {
                continue;
            }

            let url_raw = match extract_url(req_val) {
                Some(u) => u,
                None => continue,
            };

            let path_only = &url_raw[url_path_start(&url_raw)..];
            if path_only.to_lowercase().contains(pattern_lower) {
                let name = item["name"].as_str().unwrap_or("").to_string();
                let folder_path = if folder.is_empty() {
                    name.clone()
                } else {
                    format!("{folder}/{name}")
                };
                out.push(MatchedRequest {
                    name,
                    folder_path,
                    request_def: req_val,
                });
            }
        }
    }
}

// ── Tool: postman_test_endpoint ───────────────────────────────────────────

/// Test a local API endpoint by finding and executing a matching Postman
/// request. Searches all collections for a request with the given HTTP
/// method and URL path, redirects it to the local dev server, and
/// returns the response.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PostmanTestEndpointInput {
    /// HTTP method to match (GET, POST, PUT, DELETE, PATCH).
    method: String,
    /// URL path or keyword to match against request URLs (e.g. "/v1/users", "users").
    path: String,
    /// Base URL of the local server. Defaults to "http://localhost:8080".
    base_url: Option<String>,
    /// Postman environment name for variable substitution. Auto-selects if omitted.
    environment: Option<String>,
}

pub struct PostmanTestEndpointTool {
    client: Arc<PostmanClient>,
}

impl PostmanTestEndpointTool {
    pub fn new(client: Arc<PostmanClient>) -> Self {
        Self { client }
    }
}

impl AgentTool for PostmanTestEndpointTool {
    type Input = PostmanTestEndpointInput;
    type Output = String;

    const NAME: &'static str = "postman_test_endpoint";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Fetch
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        match input {
            Ok(i) => format!("Test endpoint: {} {}", i.method.to_uppercase(), i.path).into(),
            Err(_) => "Test endpoint".into(),
        }
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        let client = self.client.clone();
        cx.spawn(async move |cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive input: {e}"))?;

            let label = format!(
                "Test endpoint: {} {}",
                input.method.to_uppercase(),
                input.path
            );
            let context = crate::ToolPermissionContext::new(Self::NAME, vec![label.clone()]);
            let authorize = cx.update(|cx| event_stream.authorize(label, context, cx));
            authorize.await.map_err(|e| e.to_string())?;

            let run_task = cx.background_spawn(async move {
                // Resolve environment variables
                let mut env_vars = match client.list_environments().await {
                    Ok(envs) => {
                        let env_id = envs["environments"]
                            .as_array()
                            .and_then(|list| {
                                if let Some(name) = &input.environment {
                                    list.iter().find(|e| {
                                        e["name"].as_str().unwrap_or("") == name.as_str()
                                    })
                                } else {
                                    list.first()
                                }
                            })
                            .and_then(|e| e["uid"].as_str().or_else(|| e["id"].as_str()))
                            .map(|s| s.to_string());

                        if let Some(id) = env_id {
                            client
                                .get_environment(&id)
                                .await
                                .map(|j| parse_env_vars(&j))
                                .unwrap_or_default()
                        } else {
                            HashMap::new()
                        }
                    }
                    Err(_) => HashMap::new(),
                };

                // Inject base URL under the common Postman variable names so that
                // collections using any of these conventions are redirected correctly.
                let base = input
                    .base_url
                    .as_deref()
                    .unwrap_or("http://localhost:8080")
                    .to_string();
                for key in &["base_url", "baseUrl", "BASE_URL", "host", "baseurl"] {
                    env_vars.insert(key.to_string(), base.clone());
                }

                // Search collections for a matching request (stop at first hit)
                let collections_json = client.list_collections().await?;

                // (name, folder_path, collection_name, request_def)
                type MatchTuple = (String, String, String, serde_json::Value);
                let mut found: Option<MatchTuple> = None;

                if let Some(entries) = collections_json["collections"].as_array() {
                    for entry in entries {
                        let Some(col_id) = entry["uid"].as_str().or_else(|| entry["id"].as_str())
                        else {
                            continue;
                        };
                        let col_name = entry["name"].as_str().unwrap_or("").to_string();
                        let collection_json = client.get_collection(col_id).await?;
                        let hits = find_requests_by_endpoint(
                            &collection_json,
                            &input.method,
                            &input.path,
                        );
                        if let Some(hit) = hits.into_iter().next() {
                            found = Some((
                                hit.name,
                                hit.folder_path,
                                col_name,
                                hit.request_def.clone(),
                            ));
                            break;
                        }
                    }
                }

                let (matched_name, matched_folder, matched_collection_name, request_def) =
                    found.ok_or_else(|| {
                        anyhow::anyhow!(
                            "No Postman request matching {} {}",
                            input.method.to_uppercase(),
                            input.path
                        )
                    })?;

                let result = client
                    .execute_request_detailed(&request_def, &env_vars, Some(&base))
                    .await?;

                let status_label = if result.status < 400 { "success" } else { "error" };
                let body_kb = result.body.len() as f64 / 1024.0;
                let headers_display: String = result
                    .headers
                    .iter()
                    .map(|(k, v)| format!("  {k}: {v}"))
                    .collect::<Vec<_>>()
                    .join("\n");

                Ok(format!(
                    "Matched: {method} \"{name}\" from collection \"{col}\" ({folder})\nURL: {url}\nStatus: {status} ({status_label})\nDuration: {duration_ms}ms\nHeaders:\n{headers}\nBody ({body_kb:.1}KB):\n{body}",
                    method = input.method.to_uppercase(),
                    name = matched_name,
                    col = matched_collection_name,
                    folder = matched_folder,
                    url = result.url,
                    status = result.status,
                    duration_ms = result.duration_ms,
                    headers = headers_display,
                    body = result.body,
                ))
            });

            futures::select! {
                result = run_task.fuse() => result.map_err(|e: anyhow::Error| e.to_string()),
                _ = event_stream.cancelled_by_user().fuse() => {
                    Err("Request cancelled by user".to_string())
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_vars_replaces_known_variables() {
        let vars = HashMap::from([
            ("base_url".to_string(), "https://example.com".to_string()),
            ("token".to_string(), "abc123".to_string()),
        ]);
        assert_eq!(
            substitute_vars("{{base_url}}/api?t={{token}}", &vars),
            "https://example.com/api?t=abc123",
        );
    }

    #[test]
    fn test_substitute_vars_leaves_unknown_placeholders() {
        let vars = HashMap::new();
        let s = "{{unknown}}/path";
        assert_eq!(substitute_vars(s, &vars), s);
    }

    #[test]
    fn test_find_request_top_level() {
        let collection = serde_json::json!({
            "collection": {
                "item": [
                    {
                        "name": "GetUsers",
                        "request": { "method": "GET", "url": "https://api.example.com/users" }
                    }
                ]
            }
        });
        let req = find_request(&collection, "GetUsers").unwrap();
        assert_eq!(req["method"].as_str(), Some("GET"));
    }

    #[test]
    fn test_find_request_nested() {
        let collection = serde_json::json!({
            "collection": {
                "item": [
                    {
                        "name": "Users",
                        "item": [
                            {
                                "name": "Create",
                                "request": { "method": "POST", "url": "https://api.example.com/users" }
                            }
                        ]
                    }
                ]
            }
        });
        let req = find_request(&collection, "Users/Create").unwrap();
        assert_eq!(req["method"].as_str(), Some("POST"));
    }

    #[test]
    fn test_parse_env_vars_skips_disabled() {
        let env = serde_json::json!({
            "environment": {
                "values": [
                    { "key": "host", "value": "localhost", "enabled": true },
                    { "key": "secret", "value": "s3cr3t", "enabled": false },
                ]
            }
        });
        let vars = parse_env_vars(&env);
        assert_eq!(vars.get("host").map(|s| s.as_str()), Some("localhost"));
        assert!(!vars.contains_key("secret"));
    }
}
