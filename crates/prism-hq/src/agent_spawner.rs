use std::path::PathBuf;

use gpui::AppContext as _;
use tokio::io::AsyncBufReadExt as _;
use tokio::process::Command;

use crate::running_agents::RunningAgents;
use crate::types::prism_binary;

/// Spawn a new prism agent in a fresh git worktree.
///
/// Creates the worktree branch, then launches `prism run` as a subprocess
/// with stdout/stderr streamed into the RunningAgents ring buffer.
pub async fn spawn_agent_in_worktree(
    task_desc: String,
    thread_name: String,
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

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Register in RunningAgents before spawning so the UI shows "running" immediately.
    let agent_name_for_register = agent_name.clone();
    let mut rx_opt = Some(rx);
    cx.update(|cx| {
        if let Some(running_agents) = RunningAgents::global(cx) {
            if let Some(rx) = rx_opt.take() {
                let weak = running_agents.downgrade();
                running_agents.update(cx, |ra, cx| {
                    ra.register(agent_name_for_register, rx, weak, cx);
                });
            }
        }
    });

    // Spawn the process and stream its output to the channel.
    cx.background_spawn({
        let wt_path = wt_path.clone();
        let agent_name = agent_name.clone();
        let task_desc = task_desc.clone();
        let thread_name = thread_name.clone();
        async move {
            let mut child = Command::new(prism_binary())
                .args([
                    "run",
                    "--model",
                    "claude-sonnet-4-6",
                    &format!(
                        "You are agent '{}' assigned to thread '{}'. Task: {}",
                        agent_name, thread_name, task_desc
                    ),
                ])
                .current_dir(&wt_path)
                .env("UH_AGENT_NAME", &agent_name)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;

            let stdout = child.stdout.take().map(tokio::io::BufReader::new);
            let stderr = child.stderr.take().map(tokio::io::BufReader::new);

            let tx_stdout = tx.clone();
            let tx_stderr = tx;

            let stdout_task = async move {
                if let Some(mut reader) = stdout {
                    let mut line = String::new();
                    while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                        let trimmed = line.trim_end().to_string();
                        let _ = tx_stdout.send(trimmed);
                        line.clear();
                    }
                }
            };

            let stderr_task = async move {
                if let Some(mut reader) = stderr {
                    let mut line = String::new();
                    while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                        let trimmed = format!("[stderr] {}", line.trim_end());
                        let _ = tx_stderr.send(trimmed);
                        line.clear();
                    }
                }
            };

            tokio::join!(stdout_task, stderr_task);
            child.wait().await.ok();
            anyhow::Ok(())
        }
    })
    .detach();

    Ok(())
}
