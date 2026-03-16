use std::path::PathBuf;

use gpui::AppContext as _;

use crate::running_agents::RunningAgents;

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

/// Register the agent in RunningAgents and notify the IDE agent panel to open a new session.
///
/// The IDE agent (ide/agent) handles session creation natively via its panel. This function
/// registers the agent slot so the running-agents roster shows it, then logs that the IDE panel
/// should be used to start the actual session.
fn register_agent(agent_name: String, cx: &mut gpui::AsyncApp) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let name = agent_name.clone();
    cx.update(|cx| {
        if let Some(running_agents) = RunningAgents::global(cx) {
            let weak = running_agents.downgrade();
            running_agents.update(cx, |ra, cx| {
                ra.register(name, rx, weak, cx);
            });
        }
    });

    // Send a single status line so the roster entry doesn't appear blank.
    let _ = tx.send(format!(
        "[prism-hq] agent '{agent_name}' registered — open the Agent panel to start a session"
    ));
}
