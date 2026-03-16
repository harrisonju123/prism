use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Top-level hooks configuration loaded from `.prism/hooks.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default)]
    pub pre_tool_use: Vec<HookEntry>,
    #[serde(default)]
    pub post_tool_use: Vec<HookEntry>,
    #[serde(default)]
    pub auto_save: Option<AutoSaveConfig>,
}

/// A single hook entry that fires when a tool name matches `matcher`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEntry {
    /// Glob pattern for tool names (e.g. "bash", "edit_file*", "*")
    pub matcher: String,
    /// Shell command to run. Receives JSON payload on stdin.
    pub command: String,
    /// Timeout in milliseconds (default: 5000)
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    5000
}

/// Auto-save configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoSaveConfig {
    pub enabled: bool,
}

/// Action returned by a pre-tool hook.
#[derive(Debug)]
pub enum PreToolAction {
    Allow,
    Deny { message: String },
    Modify { args: serde_json::Value },
}

enum HookOutput {
    Ok,
    Deny(String),
    Modify(String),
}

/// Runs hooks for pre- and post-tool-use events.
pub struct HookRunner {
    config: HooksConfig,
}

impl HookRunner {
    /// Load hook config from `~/.prism/hooks.json` (user-level) then
    /// `{worktree_root}/.prism/hooks.json` (project-level, overrides user).
    /// Returns `None` if no hooks are configured.
    pub fn load(worktree_root: &Path) -> Option<Arc<Self>> {
        let mut config = HooksConfig::default();

        // User-level
        if let Some(home) = dirs::home_dir() {
            let path = home.join(".prism").join("hooks.json");
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(c) = serde_json::from_str::<HooksConfig>(&content) {
                    config = c;
                }
            }
        }

        // Project-level (overrides user-level entirely)
        let project_path = worktree_root.join(".prism").join("hooks.json");
        if let Ok(content) = std::fs::read_to_string(&project_path) {
            if let Ok(c) = serde_json::from_str::<HooksConfig>(&content) {
                config = c;
            }
        }

        if config.pre_tool_use.is_empty()
            && config.post_tool_use.is_empty()
            && config.auto_save.is_none()
        {
            return None;
        }

        Some(Arc::new(Self { config }))
    }

    pub fn auto_save_enabled(&self) -> bool {
        self.config
            .auto_save
            .as_ref()
            .map(|s| s.enabled)
            .unwrap_or(false)
    }

    /// Run pre-tool hooks synchronously. Returns `PreToolAction` indicating what to do.
    pub fn run_pre_hooks(&self, tool_name: &str, args: &serde_json::Value) -> PreToolAction {
        let args_str = serde_json::to_string(args).unwrap_or_default();
        for entry in &self.config.pre_tool_use {
            if !matches_glob(&entry.matcher, tool_name) {
                continue;
            }
            match run_hook_command(&entry.command, tool_name, &args_str, entry.timeout_ms) {
                HookOutput::Deny(msg) => return PreToolAction::Deny { message: msg },
                HookOutput::Modify(new_args) => {
                    if let Ok(v) = serde_json::from_str(&new_args) {
                        return PreToolAction::Modify { args: v };
                    }
                }
                HookOutput::Ok => {}
            }
        }
        PreToolAction::Allow
    }

    /// Run post-tool hooks synchronously. Returns the (possibly modified) result string.
    pub fn run_post_hooks(&self, tool_name: &str, result: &str) -> String {
        let mut current = result.to_string();
        for entry in &self.config.post_tool_use {
            if !matches_glob(&entry.matcher, tool_name) {
                continue;
            }
            match run_hook_command(&entry.command, tool_name, &current, entry.timeout_ms) {
                HookOutput::Modify(new_result) => current = new_result,
                HookOutput::Deny(_) | HookOutput::Ok => {}
            }
        }
        current
    }
}

/// Run a hook shell command with the given payload and timeout.
/// The payload is sent as JSON on stdin:  `{"tool_name": "...", "input": "..."}`.
///
/// Output protocol:
///   - Exit 0, stdout starts with "DENY:" → deny with message
///   - Exit 0, stdout starts with "MODIFY:" → modify args/result with remainder
///   - Exit 0, any other output → allow (passthrough)
///   - Non-zero exit or timeout → allow (passthrough)
fn run_hook_command(command: &str, tool_name: &str, input: &str, timeout_ms: u64) -> HookOutput {
    let payload = serde_json::json!({
        "tool_name": tool_name,
        "input": input,
    });
    let payload_str = payload.to_string();
    let command = command.to_string();

    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("prism-hook".into())
        .spawn(move || {
        let result = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        let output = match result {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(payload_str.as_bytes());
                }
                child.wait_with_output().ok()
            }
            Err(_) => None,
        };
        tx.send(output).ok();
    });

    let output = match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(Some(out)) if out.status.success() => out,
        _ => return HookOutput::Ok,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if let Some(msg) = trimmed.strip_prefix("DENY:") {
        HookOutput::Deny(msg.trim().to_string())
    } else if let Some(new_val) = trimmed.strip_prefix("MODIFY:") {
        HookOutput::Modify(new_val.trim().to_string())
    } else {
        HookOutput::Ok
    }
}

/// Simple glob matching: supports `*` wildcard at the start or end.
fn matches_glob(pattern: &str, name: &str) -> bool {
    if pattern == "*" || pattern == name {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_exact_match() {
        assert!(matches_glob("bash", "bash"));
        assert!(!matches_glob("bash", "bash_tool"));
    }

    #[test]
    fn glob_wildcard_all() {
        assert!(matches_glob("*", "anything"));
        assert!(matches_glob("*", ""));
    }

    #[test]
    fn glob_prefix_wildcard() {
        assert!(matches_glob("edit_*", "edit_file"));
        assert!(matches_glob("edit_*", "edit_file_tool"));
        assert!(!matches_glob("edit_*", "read_file"));
    }

    #[test]
    fn glob_suffix_wildcard() {
        assert!(matches_glob("*_tool", "edit_tool"));
        assert!(matches_glob("*_tool", "read_tool"));
        assert!(!matches_glob("*_tool", "bash"));
    }
}
