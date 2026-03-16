use std::path::PathBuf;

use gpui::AppContext as _;

use crate::running_agents::{AgentOutput, RunningAgent, RunningAgents};

/// Spawn a new prism agent in a fresh git worktree and register it in the agent roster.
///
/// Creates the worktree branch, then registers the agent in RunningAgents so the IDE
/// Agent panel can pick up the session natively.
pub async fn spawn_agent_in_worktree(
    agent_name: String,
    repo_root: PathBuf,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    let wt_path = repo_root.join(".worktrees").join(&agent_name);

    let git_status = cx
        .background_spawn({
            let repo_root = repo_root.clone();
            let agent_name = agent_name.clone();
            let wt_path = wt_path.clone();
            async move {
                std::process::Command::new("git")
                    .args(["worktree", "add"])
                    .arg(&wt_path)
                    .arg("-b")
                    .arg(&agent_name)
                    .current_dir(&repo_root)
                    .status()
            }
        })
        .await?;

    if !git_status.success() {
        anyhow::bail!("git worktree add failed");
    }

    register_agent(agent_name, cx);
    Ok(())
}

/// Register the agent slot in RunningAgents so the IDE roster shows it.
///
/// Post-consolidation, agents run natively in the IDE — there is no subprocess to stream
/// output from. We simply insert the entry so the roster is populated; the Agent panel
/// opens the actual session.
fn register_agent(agent_name: String, cx: &mut gpui::AsyncApp) {
    cx.update(|cx| {
        if let Some(running_agents) = RunningAgents::global(cx) {
            running_agents.update(cx, |ra, _cx| {
                ra.processes.insert(
                    agent_name.clone(),
                    RunningAgent {
                        agent_name,
                        output: AgentOutput::new_empty(),
                        is_running: false,
                    },
                );
            });
        }
    });
}
