use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvocationStatus {
    Success,
    Failure,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Invoke,
    Response,
    CapabilityQuery,
    CapabilityResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationRequest {
    pub caller_agent_id: String,
    pub target_listing_id: String,
    pub method: String,
    pub params: serde_json::Value,
    pub max_cost: Option<f64>,
    pub timeout_s: Option<u64>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationResponse {
    pub request_id: Uuid,
    pub status: InvocationStatus,
    pub result: serde_json::Value,
    pub cost: f64,
    pub latency_ms: u64,
    pub target_framework: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCapability {
    pub listing_id: String,
    pub methods: Vec<String>,
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
    pub supported_frameworks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolMessage {
    pub version: String,
    pub msg_type: MessageType,
    pub sender: String,
    pub receiver: String,
    pub payload: serde_json::Value,
    pub signature: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeteringRecord {
    pub invocation_id: Uuid,
    pub caller_id: String,
    pub target_id: String,
    pub tokens_used: u64,
    pub cost: f64,
    pub latency_ms: u64,
    pub timestamp: DateTime<Utc>,
}
