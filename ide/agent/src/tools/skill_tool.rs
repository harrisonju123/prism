use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task, WeakEntity};
use prism_context::skills::SkillRegistry;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, Thread, ToolCallEventStream, ToolInput};

/// Executes a named skill from the project's `.prism/skills` directory.
///
/// Skills are reusable prompt templates discovered from `.prism/skills/<name>/SKILL.md`.
/// Invoke a skill by name to inject its prompt content into the conversation.
/// After this tool returns, the skill's content will be appended as a user message.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SkillToolInput {
    /// Name of the skill to execute (e.g. "commit", "review")
    pub skill: String,
    /// Optional arguments to pass to the skill
    #[serde(default)]
    pub args: String,
}

pub struct SkillTool {
    registry: Arc<SkillRegistry>,
    thread: WeakEntity<Thread>,
}

impl SkillTool {
    pub fn new(registry: Arc<SkillRegistry>, thread: WeakEntity<Thread>) -> Self {
        Self { registry, thread }
    }
}

impl AgentTool for SkillTool {
    type Input = SkillToolInput;
    type Output = String;

    const NAME: &'static str = "skill";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        let name = match &input {
            Ok(i) => i.skill.clone(),
            Err(v) => v
                .get("skill")
                .and_then(|v| v.as_str())
                .unwrap_or("skill")
                .to_string(),
        };
        format!("Skill: {name}").into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        let thread = self.thread.clone();
        let registry = self.registry.clone();
        cx.spawn(async move |cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("failed to receive tool input: {e}"))?;
            let exec = registry.execute(&input.skill, &input.args);
            if let Some(injection) = exec.injection {
                cx.update(|cx| {
                    thread
                        .update(cx, |t, _| t.pending_skill_injections.push(injection))
                        .ok();
                });
            }
            Ok(exec.tool_result)
        })
    }
}
