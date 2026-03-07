use async_trait::async_trait;
use uuid::Uuid;

use crate::error::Result;

use super::types::{AgentCapability, InvocationRequest, InvocationResponse, InvocationStatus};

#[async_trait]
pub trait FrameworkAdapter: Send + Sync {
    fn framework_name(&self) -> &str;
    async fn invoke(&self, request: &InvocationRequest) -> Result<InvocationResponse>;
    fn capabilities(&self) -> Vec<AgentCapability>;
}

pub struct LangChainAdapter;

#[async_trait]
impl FrameworkAdapter for LangChainAdapter {
    fn framework_name(&self) -> &str {
        "langchain"
    }

    async fn invoke(&self, _request: &InvocationRequest) -> Result<InvocationResponse> {
        // Stub: in production, this would call into a LangChain runtime
        Ok(InvocationResponse {
            request_id: Uuid::new_v4(),
            status: InvocationStatus::Failure,
            result: serde_json::json!({"error": "LangChain adapter not connected"}),
            cost: 0.0,
            latency_ms: 0,
            target_framework: Some("langchain".into()),
        })
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![]
    }
}

pub struct CrewAIAdapter;

#[async_trait]
impl FrameworkAdapter for CrewAIAdapter {
    fn framework_name(&self) -> &str {
        "crewai"
    }

    async fn invoke(&self, _request: &InvocationRequest) -> Result<InvocationResponse> {
        Ok(InvocationResponse {
            request_id: Uuid::new_v4(),
            status: InvocationStatus::Failure,
            result: serde_json::json!({"error": "CrewAI adapter not connected"}),
            cost: 0.0,
            latency_ms: 0,
            target_framework: Some("crewai".into()),
        })
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![]
    }
}

pub struct AutoGenAdapter;

#[async_trait]
impl FrameworkAdapter for AutoGenAdapter {
    fn framework_name(&self) -> &str {
        "autogen"
    }

    async fn invoke(&self, _request: &InvocationRequest) -> Result<InvocationResponse> {
        Ok(InvocationResponse {
            request_id: Uuid::new_v4(),
            status: InvocationStatus::Failure,
            result: serde_json::json!({"error": "AutoGen adapter not connected"}),
            cost: 0.0,
            latency_ms: 0,
            target_framework: Some("autogen".into()),
        })
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![]
    }
}
