//! prism-cli — lightweight CLI for driving PrisM agent workflows.
//!
//! Usage:
//!   prism-cli spawn-agent --task-id <id> --worktree <path> --agent-name <name> [--model <model>]
//!
//! The `spawn-agent` subcommand:
//!   1. Creates the git worktree at <path> if it does not exist.
//!   2. Claims the uglyhat task <task-id> under agent name <name>.
//!   3. Launches `claude --dangerously-skip-permissions` in the worktree,
//!      streaming its stdout/stderr to this process's stdout.

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Parser)]
#[command(name = "prism-cli", about = "PrisM agent workflow CLI")]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    /// Spawn a headless Claude Code agent session in a git worktree.
    SpawnAgent(SpawnAgentArgs),
}

#[derive(Parser)]
struct SpawnAgentArgs {
    /// uglyhat task ID to claim before starting the agent.
    #[arg(long)]
    task_id: String,

    /// Path to the worktree directory (will be created if absent).
    #[arg(long)]
    worktree: PathBuf,

    /// Agent name used for uglyhat checkin/checkout.
    #[arg(long)]
    agent_name: String,

    /// Language model identifier to pass to Claude Code (informational).
    #[arg(long, default_value = "claude-sonnet-4-6")]
    model: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        CliCommand::SpawnAgent(args) => spawn_agent(args).await,
    }
}

async fn spawn_agent(args: SpawnAgentArgs) -> Result<()> {
    let worktree = &args.worktree;

    // Resolve the git repo root by walking up from the worktree path or its parent.
    let repo_root = find_git_root(worktree).context("Could not determine git repository root")?;

    // Step 1: create the git worktree if it does not exist.
    if !worktree.exists() {
        let branch = worktree
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("agent-worktree");
        eprintln!(
            "Creating git worktree at {} (branch: {})",
            worktree.display(),
            branch
        );
        create_worktree(&repo_root, worktree, branch).await?;
    } else {
        eprintln!("Worktree already exists at {}", worktree.display());
    }

    // Step 2: claim the uglyhat task (best-effort).
    let uh_bin = home_dir().join(".cargo/bin/uh");
    if uh_bin.exists() {
        let status = Command::new(&uh_bin)
            .args(["task", "claim", &args.task_id, "--name", &args.agent_name])
            .env("UH_AGENT_NAME", &args.agent_name)
            .current_dir(worktree)
            .status()
            .await;
        match status {
            Ok(s) if s.success() => eprintln!("Claimed task {}", args.task_id),
            Ok(s) => eprintln!(
                "Warning: uh task claim exited with status {}; continuing",
                s
            ),
            Err(e) => eprintln!("Warning: could not run uh task claim: {}; continuing", e),
        }
    } else {
        eprintln!(
            "Warning: uh binary not found at {}; skipping task claim",
            uh_bin.display()
        );
    }

    // Step 3: launch claude --dangerously-skip-permissions and stream output.
    eprintln!(
        "Launching claude agent (model: {}) in {}",
        args.model,
        worktree.display()
    );

    let mut child = Command::new("claude")
        .arg("--dangerously-skip-permissions")
        .current_dir(worktree)
        .env("UH_AGENT_NAME", &args.agent_name)
        .env("PATH", augmented_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn `claude` process — is Claude Code installed?")?;

    // Stream stdout and stderr concurrently.
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            println!("{}", line);
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("{}", line);
        }
    });

    let status = child
        .wait()
        .await
        .context("Failed to wait for claude process")?;
    stdout_task.await.ok();
    stderr_task.await.ok();

    if !status.success() {
        bail!("claude process exited with status {}", status);
    }
    Ok(())
}

/// Creates a git worktree at `path` with a new branch named `branch`.
async fn create_worktree(repo_root: &PathBuf, path: &PathBuf, branch: &str) -> Result<()> {
    let status = Command::new("git")
        .args(["worktree", "add", &path.to_string_lossy(), "-b", branch])
        .current_dir(repo_root)
        .status()
        .await;

    match status {
        Ok(s) if s.success() => return Ok(()),
        _ => {}
    }

    // Retry without -b in case the branch already exists.
    let status = Command::new("git")
        .args(["worktree", "add", &path.to_string_lossy(), branch])
        .current_dir(repo_root)
        .status()
        .await
        .context("Failed to run git worktree add")?;

    if !status.success() {
        bail!("git worktree add failed with status {}", status);
    }
    Ok(())
}

/// Walks up from `start` to find the nearest `.git` directory or file.
fn find_git_root(start: &PathBuf) -> Option<PathBuf> {
    // Start from the parent if the worktree doesn't exist yet.
    let start = if start.exists() {
        start.clone()
    } else {
        start.parent()?.to_path_buf()
    };

    let mut current = start.as_path();
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Returns a PATH string that prepends `~/.cargo/bin` to the current PATH.
fn augmented_path() -> String {
    let cargo_bin = home_dir().join(".cargo/bin");
    let existing = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", cargo_bin.display(), existing)
}
