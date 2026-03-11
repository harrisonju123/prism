use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Generate request replay artifacts from OpenAPI specs.
pub mod generate;
/// Discover or generate OpenAPI specs for request replay.
pub mod openapi;
/// Replay saved request artifacts against local/dev/staging targets.
pub mod run;

pub use generate::generate;
pub use openapi::discover_or_generate;
pub use run::run;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestReplayBundle {
    pub version: String,
    pub service: ServiceMetadata,
    pub auth: AuthSpec,
    pub base_urls: BTreeMap<String, String>,
    pub requests: Vec<RequestReplay>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMetadata {
    pub name: String,
    pub description: Option<String>,
    pub openapi_source: String,
    pub generated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSpec {
    pub schemes: Vec<AuthScheme>,
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthScheme {
    pub id: String,
    pub r#type: String,
    pub header: Option<String>,
    pub prefix: Option<String>,
    pub env_var: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestReplay {
    pub id: String,
    pub name: String,
    pub method: String,
    pub path: String,
    pub tags: Vec<String>,
    pub description: Option<String>,
    pub auth: Option<RequestAuth>,
    pub path_params: Vec<PathParam>,
    pub query: Vec<QueryParam>,
    pub headers: Vec<HeaderParam>,
    pub body: Option<BodySpec>,
    pub variants: Vec<RequestVariant>,
    pub expected: ExpectedResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestAuth {
    pub scheme_id: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathParam {
    pub name: String,
    pub required: bool,
    pub example: Option<serde_json::Value>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParam {
    pub name: String,
    pub required: bool,
    pub example: Option<serde_json::Value>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderParam {
    pub name: String,
    pub required: bool,
    pub example: Option<serde_json::Value>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BodySpec {
    pub content_type: String,
    pub example: Option<serde_json::Value>,
    pub schema_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVariant {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub request: VariantRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantRequest {
    pub query: BTreeMap<String, serde_json::Value>,
    pub headers: BTreeMap<String, serde_json::Value>,
    pub body: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub schema_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRun {
    pub request_id: String,
    pub variant_id: String,
    pub env: String,
    pub url: String,
    pub method: String,
    pub status: Option<u16>,
    pub ok: bool,
    pub latency_ms: Option<u128>,
    pub response_body: Option<serde_json::Value>,
    pub error: Option<String>,
    pub captured_logs: Vec<CapturedLog>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedLog {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub fields: BTreeMap<String, serde_json::Value>,
}
