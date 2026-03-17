use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::model::{AssumptionStatus, AutonomyLevel, InboxEntryType, InboxSeverity, MissionPhase};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Update mission state: advance phase, record assumptions/blockers, control autonomy.
/// Use this tool to manage the active plan's execution state.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateMissionToolInput {
    /// Action to perform. One of:
    /// - "advance_phase": move to the next phase in sequence
    /// - "set_phase": jump to a specific phase (requires phase field)
    /// - "add_assumption": record an assumption (requires text field)
    /// - "confirm_assumption": mark assumption confirmed (requires index field)
    /// - "reject_assumption": mark assumption rejected (requires index field)
    /// - "add_blocker": record a blocker (requires text field)
    /// - "resolve_blocker": mark blocker resolved (requires index field)
    /// - "set_autonomy": change autonomy level (requires autonomy field)
    pub action: String,
    /// Phase to jump to (for set_phase): investigate, plan, clarify, implement, validate, review, finalize
    #[serde(default)]
    pub phase: Option<String>,
    /// Text for add_assumption or add_blocker actions
    #[serde(default)]
    pub text: Option<String>,
    /// Zero-based index for confirm_assumption, reject_assumption, resolve_blocker
    #[serde(default)]
    pub index: Option<usize>,
    /// Autonomy level for set_autonomy: supervised, balanced, autonomous
    #[serde(default)]
    pub autonomy: Option<String>,
}

pub struct UpdateMissionTool {
    context: Arc<ContextHandle>,
}

impl UpdateMissionTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for UpdateMissionTool {
    type Input = UpdateMissionToolInput;
    type Output = String;

    const NAME: &'static str = "update_mission";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Update mission".into()
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

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    // Resolve the active plan.
                    let plan = context
                        .get_active_plan()
                        .await
                        .map_err(|e| anyhow::anyhow!("get_active_plan failed: {e}"))?
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "No active plan found. Create and activate a plan first."
                            )
                        })?;

                    let plan_id = plan.id;

                    match input.action.as_str() {
                        "advance_phase" => {
                            let old_phase = plan.current_phase.clone();
                            let autonomy = plan.autonomy_level.clone();
                            let next = plan
                                .current_phase
                                .next()
                                .ok_or_else(|| {
                                    anyhow::anyhow!("Already at the final phase (finalize)")
                                })?;
                            let phase_name = next.to_string();
                            let old_phase_name = old_phase.to_string();
                            context
                                .update_plan_phase(plan_id, next.clone())
                                .await
                                .map_err(|e| anyhow::anyhow!("advance_phase failed: {e}"))?;

                            // Autonomy-gated: supervised always pauses, balanced pauses at key transitions
                            let requires_pause = match autonomy {
                                AutonomyLevel::Autonomous => false,
                                AutonomyLevel::Supervised => true,
                                AutonomyLevel::Balanced => matches!(
                                    (&old_phase, &next),
                                    (MissionPhase::Investigate, MissionPhase::Plan)
                                        | (MissionPhase::Clarify, MissionPhase::Implement)
                                        | (MissionPhase::Implement, MissionPhase::Validate)
                                        | (MissionPhase::Review, MissionPhase::Finalize)
                                ),
                            };

                            if requires_pause {
                                let title =
                                    format!("Phase transition: {old_phase_name} → {phase_name}");
                                let body = format!(
                                    "Agent advanced mission phase from **{old_phase_name}** to \
                                     **{phase_name}**. Review and respond to continue."
                                );
                                let _ = context
                                    .create_inbox_entry(
                                        InboxEntryType::Approval,
                                        &title,
                                        &body,
                                        InboxSeverity::Warning,
                                        None,
                                        None,
                                    )
                                    .await;
                                Ok(format!(
                                    "Phase advanced to {phase_name}. Approval required — inbox \
                                     notification created. Stop and await user response."
                                ))
                            } else {
                                Ok(format!("Phase advanced to: {phase_name}"))
                            }
                        }
                        "set_phase" => {
                            let phase_str = input
                                .phase
                                .ok_or_else(|| anyhow::anyhow!("phase field required for set_phase"))?;
                            let new_phase = MissionPhase::from_str(&phase_str).ok_or_else(|| {
                                anyhow::anyhow!("unknown phase: {phase_str}")
                            })?;
                            let phase_name = new_phase.to_string();
                            let old_phase_name = plan.current_phase.to_string();
                            let autonomy = plan.autonomy_level.clone();
                            context
                                .update_plan_phase(plan_id, new_phase)
                                .await
                                .map_err(|e| anyhow::anyhow!("set_phase failed: {e}"))?;

                            // Supervised always requires approval; balanced + autonomous proceed freely
                            if autonomy == AutonomyLevel::Supervised {
                                let title = format!("Phase set: {old_phase_name} → {phase_name}");
                                let body = format!(
                                    "Agent explicitly set mission phase from **{old_phase_name}** \
                                     to **{phase_name}**. Review and respond to continue."
                                );
                                let _ = context
                                    .create_inbox_entry(
                                        InboxEntryType::Approval,
                                        &title,
                                        &body,
                                        InboxSeverity::Warning,
                                        None,
                                        None,
                                    )
                                    .await;
                                Ok(format!(
                                    "Phase set to {phase_name}. Approval required — inbox \
                                     notification created. Stop and await user response."
                                ))
                            } else {
                                Ok(format!("Phase set to: {phase_name}"))
                            }
                        }
                        "add_assumption" => {
                            let text = input
                                .text
                                .ok_or_else(|| anyhow::anyhow!("text field required for add_assumption"))?;
                            let text_ret = text.clone();
                            context
                                .add_plan_assumption(plan_id, &text)
                                .await
                                .map_err(|e| anyhow::anyhow!("add_assumption failed: {e}"))?;
                            Ok(format!("Assumption recorded: {text_ret}"))
                        }
                        "confirm_assumption" => {
                            let index = input.index.ok_or_else(|| {
                                anyhow::anyhow!("index field required for confirm_assumption")
                            })?;
                            context
                                .update_plan_assumption(plan_id, index, AssumptionStatus::Confirmed)
                                .await
                                .map_err(|e| anyhow::anyhow!("confirm_assumption failed: {e}"))?;
                            Ok(format!("Assumption {index} confirmed"))
                        }
                        "reject_assumption" => {
                            let index = input.index.ok_or_else(|| {
                                anyhow::anyhow!("index field required for reject_assumption")
                            })?;
                            context
                                .update_plan_assumption(plan_id, index, AssumptionStatus::Rejected)
                                .await
                                .map_err(|e| anyhow::anyhow!("reject_assumption failed: {e}"))?;
                            Ok(format!("Assumption {index} rejected"))
                        }
                        "add_blocker" => {
                            let text = input
                                .text
                                .ok_or_else(|| anyhow::anyhow!("text field required for add_blocker"))?;
                            let text_ret = text.clone();
                            context
                                .add_plan_blocker(plan_id, &text)
                                .await
                                .map_err(|e| anyhow::anyhow!("add_blocker failed: {e}"))?;
                            Ok(format!("Blocker recorded: {text_ret}"))
                        }
                        "resolve_blocker" => {
                            let index = input.index.ok_or_else(|| {
                                anyhow::anyhow!("index field required for resolve_blocker")
                            })?;
                            context
                                .resolve_plan_blocker(plan_id, index)
                                .await
                                .map_err(|e| anyhow::anyhow!("resolve_blocker failed: {e}"))?;
                            Ok(format!("Blocker {index} resolved"))
                        }
                        "set_autonomy" => {
                            let autonomy_str = input.autonomy.ok_or_else(|| {
                                anyhow::anyhow!("autonomy field required for set_autonomy")
                            })?;
                            let autonomy =
                                AutonomyLevel::from_str(&autonomy_str).ok_or_else(|| {
                                    anyhow::anyhow!(
                                        "unknown autonomy level: {autonomy_str}. Use: supervised, balanced, autonomous"
                                    )
                                })?;
                            let autonomy_name = autonomy.to_string();
                            context
                                .update_plan_metadata(plan_id, None, None, Some(autonomy))
                                .await
                                .map_err(|e| anyhow::anyhow!("set_autonomy failed: {e}"))?;
                            Ok(format!("Autonomy level set to: {autonomy_name}"))
                        }
                        other => Err(anyhow::anyhow!(
                            "Unknown action: {other}. Valid actions: advance_phase, set_phase, \
                             add_assumption, confirm_assumption, reject_assumption, \
                             add_blocker, resolve_blocker, set_autonomy"
                        )),
                    }
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())
        })
    }
}
