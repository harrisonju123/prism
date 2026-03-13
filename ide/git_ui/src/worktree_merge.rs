use anyhow::{Context as _, Result, bail};
use gpui::{App, AppContext as _, Task};
use std::{path::PathBuf, time::Duration};

/// Result of a worktree merge attempt.
#[derive(Debug, Clone)]
pub enum MergeOutcome {
    /// Merge succeeded; worktree branch has been merged into target and worktree removed.
    Merged { branch: String },
    /// Merge failed with conflict markers; worktree left intact, task marked in_progress again.
    Conflict { branch: String, details: String },
    /// Merge was declined by the user.
    Rejected { branch: String },
    /// User requested changes before merging.
    ChangesRequested { branch: String, message: String },
}

/// Information about a worktree ready for review.
#[derive(Debug, Clone)]
pub struct WorktreeMergeRequest {
    /// The worktree path (absolute).
    pub worktree_path: PathBuf,
    /// Branch name of the worktree.
    pub branch: String,
    /// The context thread ID that completed.
    pub task_id: String,
    /// Task name for display.
    pub task_name: String,
    /// Task description for the approval gate.
    pub task_description: String,
    /// Session cost if available.
    pub session_cost_usd: Option<f64>,
    /// Brief test summary (e.g. "cargo test: 42 passed, 0 failed").
    pub test_summary: Option<String>,
}

/// Compute a diff summary between a worktree branch and the target branch.
///
/// Returns the diff as a string suitable for the approval gate preview.
pub fn compute_diff(repo_root: &std::path::Path, branch: &str, target: &str) -> Result<String> {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "diff",
            "--stat",
            &format!("{target}...{branch}"),
        ])
        .output()
        .context("failed to run git diff --stat")?;

    let stat = String::from_utf8_lossy(&output.stdout).to_string();

    let diff_output = std::process::Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "diff",
            &format!("{target}...{branch}"),
        ])
        .output()
        .context("failed to run git diff")?;

    let diff = String::from_utf8_lossy(&diff_output.stdout).to_string();

    Ok(format!("{stat}\n{diff}"))
}

/// Attempt to merge a worktree branch into the target branch (default: `main`).
///
/// On success the worktree is removed. On conflict the worktree is left intact
/// and the task is reset to `in_progress`.
pub fn merge_worktree(
    repo_root: &std::path::Path,
    request: &WorktreeMergeRequest,
    target_branch: &str,
) -> Result<MergeOutcome> {
    let root = repo_root.to_string_lossy();
    let branch = &request.branch;
    let worktree_path = request.worktree_path.to_string_lossy().to_string();

    // Checkout target branch
    let checkout = std::process::Command::new("git")
        .args(["-C", &root, "checkout", target_branch])
        .output()
        .context("git checkout failed")?;
    if !checkout.status.success() {
        bail!(
            "git checkout {target_branch} failed: {}",
            String::from_utf8_lossy(&checkout.stderr)
        );
    }

    // Merge the worktree branch with --no-ff for a clean history
    let merge = std::process::Command::new("git")
        .args([
            "-C",
            &root,
            "merge",
            "--no-ff",
            branch,
            "-m",
            &format!("chore: merge {branch} (context thread {})", request.task_id),
        ])
        .output()
        .context("git merge failed")?;

    if merge.status.success() {
        // Remove worktree
        let _ = std::process::Command::new("git")
            .args(["-C", &root, "worktree", "remove", "--force", &worktree_path])
            .output();

        // Delete the branch
        let _ = std::process::Command::new("git")
            .args(["-C", &root, "branch", "-d", branch])
            .output();

        return Ok(MergeOutcome::Merged {
            branch: branch.clone(),
        });
    }

    // Merge failed — likely conflicts. Abort and return conflict info.
    let _ = std::process::Command::new("git")
        .args(["-C", &root, "merge", "--abort"])
        .output();

    let conflict_details = String::from_utf8_lossy(&merge.stderr).to_string();
    Ok(MergeOutcome::Conflict {
        branch: branch.clone(),
        details: conflict_details,
    })
}

/// Remove a worktree and its branch without merging (for rejections).
pub fn remove_worktree(repo_root: &std::path::Path, request: &WorktreeMergeRequest) -> Result<()> {
    let root = repo_root.to_string_lossy();
    let worktree_path = request.worktree_path.to_string_lossy().to_string();

    let remove = std::process::Command::new("git")
        .args(["-C", &root, "worktree", "remove", "--force", &worktree_path])
        .output()
        .context("git worktree remove failed")?;

    if !remove.status.success() {
        bail!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&remove.stderr)
        );
    }

    // Best-effort branch delete
    let _ = std::process::Command::new("git")
        .args(["-C", &root, "branch", "-D", &request.branch])
        .output();

    Ok(())
}

/// Poll prism context for threads that have transitioned to `done` and have an associated
/// worktree branch. Returns a list of merge requests ready for review.
///
/// This is designed to be called on a background thread and polled on an interval.
pub fn poll_completed_worktrees(repo_root: &std::path::Path) -> Result<Vec<WorktreeMergeRequest>> {
    // Get active worktrees from git
    let worktree_output = std::process::Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "worktree",
            "list",
            "--porcelain",
        ])
        .output()
        .context("git worktree list failed")?;

    let worktree_text = String::from_utf8_lossy(&worktree_output.stdout);
    let mut worktrees: Vec<(PathBuf, String)> = Vec::new();

    let mut current_path: Option<PathBuf> = None;
    for line in worktree_text.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path_str.trim()));
        } else if let Some(branch_str) = line.strip_prefix("branch refs/heads/") {
            if let Some(path) = current_path.take() {
                worktrees.push((path, branch_str.trim().to_string()));
            }
        }
    }

    // Query prism context for done threads assigned to agents with matching worktrees
    let tasks_output = std::process::Command::new(crate::prism_binary())
        .args(["context", "thread", "list", "--archived"])
        .output()
        .context("prism context thread list failed")?;

    if !tasks_output.status.success() {
        return Ok(Vec::new());
    }

    let tasks: Vec<serde_json::Value> =
        serde_json::from_slice(&tasks_output.stdout).unwrap_or_default();

    let mut requests = Vec::new();
    for task in tasks {
        let task_id = task.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        let task_name = task
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled task");
        let description = task
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let assignee = task
            .get("assignee")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        // Look for a worktree whose branch matches the agent name or task id pattern
        for (worktree_path, branch) in &worktrees {
            if branch.contains(assignee) || branch.contains(task_id) {
                requests.push(WorktreeMergeRequest {
                    worktree_path: worktree_path.clone(),
                    branch: branch.clone(),
                    task_id: task_id.to_string(),
                    task_name: task_name.to_string(),
                    task_description: description.to_string(),
                    session_cost_usd: None,
                    test_summary: None,
                });
                break;
            }
        }
    }

    Ok(requests)
}

/// Spawn a background polling task that checks for completed worktrees every
/// `interval` and invokes `on_ready` for each one requiring review.
///
/// The returned `Task` must be stored to keep the polling alive.
pub fn spawn_merge_poller<F>(
    repo_root: PathBuf,
    interval: Duration,
    on_ready: F,
    cx: &mut App,
) -> Task<()>
where
    F: Fn(WorktreeMergeRequest, &mut App) + 'static,
{
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(interval).await;

            let root = repo_root.clone();
            let poll_result = cx
                .background_spawn(async move { poll_completed_worktrees(&root) })
                .await;

            if let Ok(requests) = poll_result {
                for request in requests {
                    let key = format!("{}-{}", request.task_id, request.branch);
                    if !seen.contains(&key) {
                        seen.insert(key);
                        cx.update(|cx| on_ready(request, cx));
                    }
                }
            }
        }
    })
}
