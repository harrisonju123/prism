use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::model::RiskSeverity;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

fn default_category() -> String {
    "general".to_string()
}

fn default_severity() -> String {
    "medium".to_string()
}

/// Flag a risk that could affect the current work.
/// Risks are tracked in the Risk Register and surfaced in HQ.
/// High-severity risks automatically create an inbox notification.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct FlagRiskToolInput {
    /// Short title describing the risk (e.g. "Auth token stored in localStorage")
    pub title: String,
    /// Detailed description of the risk and its potential impact
    #[serde(default)]
    pub description: String,
    /// Category: "integration", "assumption", "contract", "performance", "security", "general"
    #[serde(default = "default_category")]
    pub category: String,
    /// Severity: "high", "medium", or "low"
    #[serde(default = "default_severity")]
    pub severity: String,
    /// Optional tags
    #[serde(default)]
    pub tags: Vec<String>,
}

pub struct FlagRiskTool {
    context: Arc<ContextHandle>,
}

impl FlagRiskTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for FlagRiskTool {
    type Input = FlagRiskToolInput;
    type Output = String;

    const NAME: &'static str = "flag_risk";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Flag risk".into()
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

            let severity = RiskSeverity::from_str(&input.severity)
                .ok_or_else(|| format!("Invalid severity {:?}; use high, medium, or low", input.severity))?;
            let title = input.title.clone();
            let title_ret = title.clone();

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    context
                        .create_risk(
                            &title,
                            &input.description,
                            &input.category,
                            severity,
                            input.tags,
                        )
                        .await
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!("flag_risk failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())?;

            Ok(format!("Risk flagged: {title_ret}"))
        })
    }
}
