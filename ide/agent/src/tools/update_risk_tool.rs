use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::model::RiskStatus;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Update the status of a risk in the Risk Register.
/// Use this when you have mitigated, verified, or accepted a risk.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateRiskToolInput {
    /// Risk ID (UUID)
    pub risk_id: String,
    /// New status: "identified", "acknowledged", "mitigated", "verified", or "accepted"
    pub status: String,
    /// How the risk was mitigated (required when status = "mitigated")
    #[serde(default)]
    pub mitigation: String,
    /// Criteria that verify the mitigation worked (optional)
    #[serde(default)]
    pub verification_criteria: String,
}

pub struct UpdateRiskTool {
    context: Arc<ContextHandle>,
}

impl UpdateRiskTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for UpdateRiskTool {
    type Input = UpdateRiskToolInput;
    type Output = String;

    const NAME: &'static str = "update_risk";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Update risk".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        let context = self.context.clone();
        cx.spawn(async move |cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive tool input: {e}"))?;

            let risk_id: Uuid = input
                .risk_id
                .parse()
                .map_err(|e| format!("Invalid risk_id: {e}"))?;

            let status = RiskStatus::from_str(&input.status)
                .ok_or_else(|| format!("Invalid status: {}", input.status))?;

            let mitigation_owned = if input.mitigation.is_empty() { None } else { Some(input.mitigation.clone()) };
            let verification_owned = if input.verification_criteria.is_empty() {
                None
            } else {
                Some(input.verification_criteria.clone())
            };

            let status_str = input.status.clone();
            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    context
                        .update_risk_status(
                            risk_id,
                            status,
                            mitigation_owned.as_deref(),
                            verification_owned.as_deref(),
                        )
                        .await
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!("update_risk failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())?;

            Ok(format!("Risk {risk_id} updated to {status_str}"))
        })
    }
}
