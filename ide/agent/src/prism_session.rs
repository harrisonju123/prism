use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const PRISM_SESSION_FILE: &str = ".prism-session.json";
const SESSION_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Metadata written to `.prism-session.json` in the worktree root.
///
/// Other tools (uglyhat CLI hooks, status bar, etc.) read this file to learn
/// what agent is working in the worktree and which task it has claimed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrismSessionFile {
    /// Zed thread session ID
    pub session_id: String,
    /// uglyhat agent name (`UH_AGENT_NAME` env var, or "claude" by default)
    pub agent_name: String,
    /// uglyhat task ID claimed for this session, if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// uglyhat task name, for display purposes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_name: Option<String>,
    /// Absolute path of the worktree root
    pub worktree_path: String,
    /// Git branch name at session start
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Model provider/id string, e.g. "prism/claude-opus-4-6"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Session start time
    pub started_at: DateTime<Utc>,
    /// Last time the file was updated
    pub updated_at: DateTime<Utc>,
}

impl PrismSessionFile {
    pub fn new(
        session_id: String,
        agent_name: String,
        worktree_path: String,
        branch: Option<String>,
        model: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            session_id,
            agent_name,
            task_id: None,
            task_name: None,
            worktree_path,
            branch,
            model,
            started_at: now,
            updated_at: now,
        }
    }

    /// Returns true when the session file is still fresh (< 24h old).
    pub fn is_fresh(&self) -> bool {
        let age = Utc::now().signed_duration_since(self.updated_at);
        age.num_seconds() < SESSION_MAX_AGE.as_secs() as i64
    }

    /// Write the session file synchronously.  Errors are logged and ignored so
    /// that a failure here never blocks the UI.
    pub fn write_to(&self, path: &Path) {
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(err) = std::fs::write(path, json) {
                    log::warn!("Failed to write prism session file at {path:?}: {err}");
                }
            }
            Err(err) => log::warn!("Failed to serialize prism session: {err}"),
        }
    }

    /// Delete the session file.  Errors are logged and ignored.
    pub fn delete_at(path: &Path) {
        if let Err(err) = std::fs::remove_file(path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                log::warn!("Failed to delete prism session file at {path:?}: {err}");
            }
        }
    }

    /// Try to read an existing session file.  Returns `None` if the file is
    /// missing, malformed, or older than 24 hours.
    pub fn try_read_from(path: &Path) -> Option<Self> {
        let contents = std::fs::read_to_string(path).ok()?;
        let session: Self = serde_json::from_str(&contents).ok()?;
        if session.is_fresh() {
            Some(session)
        } else {
            None
        }
    }
}

/// Derive the path for `.prism-session.json` from a worktree root path.
pub fn session_file_path(worktree_root: &Path) -> PathBuf {
    worktree_root.join(PRISM_SESSION_FILE)
}

/// Read `UH_AGENT_NAME` from the environment, falling back to `"claude"`.
pub fn agent_name_from_env() -> String {
    std::env::var("PRISM_AGENT_NAME").or_else(|_| std::env::var("UH_AGENT_NAME")).unwrap_or_else(|_| "claude".to_string())
}

fn uh_binary() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let uh = PathBuf::from(home).join(".cargo/bin/uh");
    if uh.exists() { Some(uh) } else { None }
}

/// Attempt to auto-claim the uglyhat task whose ID is stored in `session`.
///
/// Invokes `~/.cargo/bin/uh task claim <id> --name <agent>` in a background
/// thread.  Returns the task name on success so the caller can update the
/// session file.
pub fn auto_claim_task(task_id: &str, agent_name: &str) -> Option<String> {
    let uh = uh_binary()?;

    let output = std::process::Command::new(&uh)
        .args(["task", "claim", task_id, "--name", agent_name])
        .output()
        .ok()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse JSON to get the task name
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout) {
            let name = value
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            return name;
        }
    } else {
        log::debug!(
            "uh task claim failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    None
}

/// Look up the uglyhat task whose branch matches `branch_name`.
///
/// Invokes `~/.cargo/bin/uh tasks` and searches by domain_tags or description.
/// Returns `(task_id, task_name)` when a match is found.
pub fn find_task_for_branch(branch_name: &str) -> Option<(String, String)> {
    let uh = uh_binary()?;

    let output = std::process::Command::new(&uh)
        .args(["tasks", "--status", "pending"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let tasks: serde_json::Value = serde_json::from_str(&stdout).ok()?;
    let task_array = tasks.as_array()?;

    // Normalise branch name for comparison: replace slashes with hyphens and
    // lowercase everything.
    let normalised_branch = branch_name.replace('/', "-").to_lowercase();

    for task in task_array {
        let id = task.get("id").and_then(|v| v.as_str())?;
        let name = task.get("name").and_then(|v| v.as_str())?;

        // Match against domain_tags array
        if let Some(tags) = task.get("domain_tags").and_then(|v| v.as_array()) {
            for tag in tags {
                if let Some(tag_str) = tag.as_str() {
                    if tag_str.replace('/', "-").to_lowercase() == normalised_branch {
                        return Some((id.to_string(), name.to_string()));
                    }
                }
            }
        }
    }
    None
}
