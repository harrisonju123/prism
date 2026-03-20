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

        // Headers
        if let Some(headers) = request_def["header"].as_array() {
            for h in headers {
                let key = h["key"].as_str().unwrap_or("");
                let val = h["value"].as_str().unwrap_or("");
                if !key.is_empty() {
                    builder = builder.header(key, substitute_vars(val, env_vars));
                }
            }
        }

        // Body
        let body = build_body(request_def, env_vars);
        let request = builder.body(body)?;

        let start = Instant::now();
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
        let body_text = String::from_utf8_lossy(&resp_body).into_owned();

        // Collect response headers
        let resp_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
            .collect();

        // Pretty-print JSON body if possible
        let body_display = serde_json::from_str::<serde_json::Value>(&body_text)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or(body_text);

        Ok(format!(
            "Status: {status}\nDuration: {elapsed_ms}ms\nHeaders: {headers_json}\n\nBody:\n{body_display}",
            headers_json = serde_json::to_string(&resp_headers).unwrap_or_default(),
        ))
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
