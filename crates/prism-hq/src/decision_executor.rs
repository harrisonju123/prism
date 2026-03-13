use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use prism_context::model::{AgentState, WorkPackageStatus};
use uuid::Uuid;

use crate::approval_gate::ApprovalDecision;
use crate::context_service::ContextHandle;

fn resolve_inbox(handle: &ContextHandle, entry_id: Option<Uuid>, resolution: &str) {
    if let Some(id) = entry_id {
        if let Err(e) = handle.resolve_inbox_entry(id, resolution) {
            log::warn!("Failed to resolve inbox entry {id}: {e}");
        }
    }
}

pub enum DecisionResult {
    Ok,
    MergeConflict { details: String },
    Error { message: String },
}

/// Discover the git repo root from cwd.
pub fn discover_repo_root() -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Some(PathBuf::from(path))
    } else {
        None
    }
}

/// Execute the decision side-effects.
///
/// - `entry_id`: inbox entry to resolve (None when triggered from AgentView)
/// - `branch`: the worktree branch name (== agent_name by convention)
/// - `agent_name`: agent whose state to update
/// - `repo_root`: if None, auto-discovered via git
pub fn execute_decision(
    decision: ApprovalDecision,
    handle: ContextHandle,
    branch: String,
    entry_id: Option<Uuid>,
    agent_name: Option<String>,
    repo_root: Option<PathBuf>,
) -> DecisionResult {
    match decision {
        ApprovalDecision::Approve => {
            execute_approve(handle, branch, entry_id, agent_name, repo_root)
        }
        ApprovalDecision::RequestChanges { message } => {
            execute_request_changes(handle, branch, entry_id, agent_name, message)
        }
        ApprovalDecision::Reject => {
            execute_reject(handle, branch, entry_id, agent_name, repo_root)
        }
    }
}

fn execute_approve(
    handle: ContextHandle,
    branch: String,
    entry_id: Option<Uuid>,
    agent_name: Option<String>,
    repo_root: Option<PathBuf>,
) -> DecisionResult {
    resolve_inbox(&handle, entry_id, r#"{"decision":"approve"}"#);

    // 2. Discover repo root.
    let root = match repo_root.or_else(discover_repo_root) {
        Some(r) => r,
        None => {
            return DecisionResult::Error {
                message: "Could not determine git repo root".to_string(),
            };
        }
    };

    // 3. git checkout main && git merge --no-ff <branch>
    match git_merge_no_ff(&root, &branch) {
        Ok(()) => {}
        Err(conflict_details) => {
            // Abort and leave worktree intact.
            let _ = std::process::Command::new("git")
                .args(["merge", "--abort"])
                .current_dir(&root)
                .status();
            // Put agent back to working so it can be retried.
            if let Some(ref name) = agent_name {
                let _ = handle.set_agent_state(name, AgentState::Working);
            }
            return DecisionResult::MergeConflict {
                details: conflict_details,
            };
        }
    }

    // 4. Post-merge cleanup (best-effort).
    let _ = handle.archive_thread(&branch);

    if let Some(ref name) = agent_name {
        if let Some(wp) = find_work_package(&handle, name) {
            let _ = handle.update_work_package_status(wp.id, WorkPackageStatus::Done);
        }
        let _ = handle.set_agent_state(name, AgentState::Idle);
    }

    git_remove_worktree(&root, &branch);

    DecisionResult::Ok
}

fn execute_request_changes(
    handle: ContextHandle,
    branch: String,
    entry_id: Option<Uuid>,
    agent_name: Option<String>,
    message: String,
) -> DecisionResult {
    let resolution = serde_json::json!({"decision": "request_changes", "message": message}).to_string();
    resolve_inbox(&handle, entry_id, &resolution);

    // 2. Save feedback as a memory on the thread.
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let key = format!("review_feedback_{ts}");
    if let Err(e) = handle.save_memory(
        &key,
        &message,
        Some(&branch),
        vec!["review-feedback".to_string()],
    ) {
        log::warn!("Failed to save review feedback memory: {e}");
    }

    // 3. Set agent back to Working so the CLI poll loop picks it up.
    if let Some(ref name) = agent_name {
        if let Err(e) = handle.set_agent_state(name, AgentState::Working) {
            log::warn!("Failed to set agent state to Working: {e}");
        }
    }

    // Note: for await_review agents the CLI poll loop reads the resolution JSON and
    // re-enters its loop automatically. For non-await-review agents (already exited)
    // manual re-spawn is required — a future enhancement can automate this.

    DecisionResult::Ok
}

fn execute_reject(
    handle: ContextHandle,
    branch: String,
    entry_id: Option<Uuid>,
    agent_name: Option<String>,
    repo_root: Option<PathBuf>,
) -> DecisionResult {
    resolve_inbox(&handle, entry_id, r#"{"decision":"reject"}"#);
    // Dismiss so it drops out of the feed.
    if let Some(id) = entry_id {
        if let Err(e) = handle.dismiss_inbox_entry(id) {
            log::warn!("Failed to dismiss inbox entry {id}: {e}");
        }
    }

    // 2. Set agent state to Dead and update WP.
    if let Some(ref name) = agent_name {
        if let Err(e) = handle.set_agent_state(name, AgentState::Dead) {
            log::warn!("Failed to set agent state to Dead: {e}");
        }
        if let Some(wp) = find_work_package(&handle, name) {
            let _ = handle.update_work_package_status(wp.id, WorkPackageStatus::Cancelled);
        }
    }

    // 3. Remove worktree + force-delete branch.
    let root = repo_root.or_else(discover_repo_root);
    if let Some(root) = root {
        git_remove_worktree(&root, &branch);
        // Force delete since not merged.
        let _ = std::process::Command::new("git")
            .args(["branch", "-D", &branch])
            .current_dir(&root)
            .status();
    } else {
        log::warn!("Could not determine repo root; skipping worktree/branch cleanup for {branch}");
    }

    DecisionResult::Ok
}

/// Checkout main and merge the branch with --no-ff.
/// Returns Ok on success, Err(details) on merge conflict or git failure.
fn git_merge_no_ff(repo_root: &PathBuf, branch: &str) -> Result<(), String> {
    // Checkout main.
    let status = std::process::Command::new("git")
        .args(["checkout", "main"])
        .current_dir(repo_root)
        .status()
        .map_err(|e| format!("git checkout main: {e}"))?;
    if !status.success() {
        return Err("git checkout main failed".to_string());
    }

    // Merge --no-ff.
    let output = std::process::Command::new("git")
        .args(["merge", "--no-ff", branch])
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("git merge: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(stderr)
    }
}

/// Remove the worktree directory and delete the local branch (normal delete).
fn git_remove_worktree(repo_root: &PathBuf, branch: &str) {
    let wt_path = repo_root.join(".worktrees").join(branch);
    let status = std::process::Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt_path)
        .current_dir(repo_root)
        .status();
    if let Err(e) = status {
        log::warn!("git worktree remove failed for {branch}: {e}");
    }

    let status = std::process::Command::new("git")
        .args(["branch", "-d", branch])
        .current_dir(repo_root)
        .status();
    if let Err(e) = status {
        log::warn!("git branch -d {branch} failed: {e}");
    }
}

/// Find an InProgress WorkPackage assigned to `agent_name`.
fn find_work_package(
    handle: &ContextHandle,
    agent_name: &str,
) -> Option<prism_context::model::WorkPackage> {
    let wps = handle
        .list_work_packages(None, Some(WorkPackageStatus::InProgress))
        .unwrap_or_default();
    wps.into_iter()
        .find(|wp| wp.assigned_agent.as_deref() == Some(agent_name))
}
