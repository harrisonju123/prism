use std::path::PathBuf;
use std::sync::mpsc;

use gpui::AppContext as _;
use gpui_tokio::Tokio;
use tokio::io::AsyncBufReadExt as _;
use tokio::process::Command;

use crate::running_agents::RunningAgents;
use crate::types::prism_binary;

/// Request to re-spawn an agent in its existing worktree after RequestChanges.
pub struct RespawnRequest {
    pub feedback: String,
    pub thread_name: String,
    pub agent_name: String,
    pub wt_path: PathBuf,
}

/// A channel sender for respawn requests. Callers on background threads send to this;
/// a GPUI task on the main thread drains it and calls `spawn_agent_in_existing_worktree`.
pub type RespawnSender = mpsc::SyncSender<RespawnRequest>;

/// Spawn a new prism agent in a fresh git worktree.
///
/// Creates the worktree branch, then launches `prism run` as a subprocess
/// with stdout/stderr streamed into the RunningAgents ring buffer.
///
/// `acceptance_criteria` is an optional list of done-conditions passed into
/// the agent prompt so it knows what "done" looks like for this work package.
pub async fn spawn_agent_in_worktree(
    task_desc: String,
    thread_name: String,
    agent_name: String,
    repo_root: PathBuf,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    spawn_agent_in_worktree_with_criteria(task_desc, thread_name, agent_name, repo_root, vec![], cx)
        .await
}

pub async fn spawn_agent_in_worktree_with_criteria(
    task_desc: String,
    thread_name: String,
    agent_name: String,
    repo_root: PathBuf,
    acceptance_criteria: Vec<String>,
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

    let criteria_text = if acceptance_criteria.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nAcceptance criteria:\n{}",
            acceptance_criteria
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let prompt = format!(
        "You are agent '{}' assigned to thread '{}'. Task: {}{}",
        agent_name, thread_name, task_desc, criteria_text
    );

    launch_prism_subprocess(wt_path, agent_name, prompt, cx);
    Ok(())
}

/// Re-spawn a prism agent in an **existing** worktree after RequestChanges review.
///
/// Unlike `spawn_agent_in_worktree_with_criteria`, this skips `git worktree add` because the
/// worktree already exists. The `feedback` string is included in the prompt so the agent
/// knows what changes to make.
pub async fn spawn_agent_in_existing_worktree(
    feedback: String,
    thread_name: String,
    agent_name: String,
    wt_path: PathBuf,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    let prompt = format!(
        "You are agent '{}' assigned to thread '{}'. \
        A reviewer has requested the following changes:\n\n{}\n\n\
        Please address the feedback and signal completion when done.",
        agent_name, thread_name, feedback
    );

    launch_prism_subprocess(wt_path, agent_name, prompt, cx);
    Ok(())
}

/// Register the agent in RunningAgents and spawn `prism run` with the given prompt,
/// streaming its stdout/stderr into the ring buffer.
fn launch_prism_subprocess(
    wt_path: PathBuf,
    agent_name: String,
    prompt: String,
    cx: &mut gpui::AsyncApp,
) {
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
    // Must run on Tokio's thread pool — tokio::process::Command requires a Tokio reactor.
    Tokio::spawn_result(cx, async move {
        let mut child = Command::new(prism_binary())
            .args(["run", "--model", "claude-sonnet-4-6", &prompt])
            .current_dir(&wt_path)
            .env("PRISM_AGENT_NAME", &agent_name)
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
    })
    .detach();
}
