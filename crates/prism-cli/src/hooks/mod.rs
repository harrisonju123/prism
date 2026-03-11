pub mod config;

use config::{HookEntry, HooksConfig, PostToolAction, PreToolAction};
use serde_json::Value;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub struct HookRunner {
    config: HooksConfig,
}

impl HookRunner {
    pub fn new(config: HooksConfig) -> Self {
        Self { config }
    }

    /// Run all matching pre-tool-use hooks. Returns the final action:
    /// - Any Deny short-circuits immediately
    /// - Modify updates args cumulatively
    /// - Otherwise Allow
    pub async fn run_pre_hooks(&self, tool_name: &str, args: &Value) -> PreToolAction {
        if self.config.pre_tool_use.is_empty() {
            return PreToolAction::Allow;
        }

        let mut current_args = args.clone();

        for hook in &self.config.pre_tool_use {
            if !matches_tool(hook, tool_name) {
                continue;
            }

            let input = serde_json::json!({
                "hook": "pre_tool_use",
                "tool_name": tool_name,
                "args": current_args,
            });

            match run_hook_process(&hook.command, &input, hook.timeout_secs).await {
                Ok(stdout) => match serde_json::from_str::<PreToolAction>(&stdout) {
                    Ok(PreToolAction::Deny { message }) => {
                        return PreToolAction::Deny { message };
                    }
                    Ok(PreToolAction::Modify { args: new_args }) => {
                        current_args = new_args;
                    }
                    Ok(PreToolAction::Allow) => {}
                    Err(e) => {
                        tracing::warn!(
                            hook = hook.command,
                            "pre-hook returned unparseable output, treating as allow: {e}"
                        );
                    }
                },
                Err(e) => {
                    if hook.fail_open {
                        tracing::warn!(
                            hook = hook.command,
                            "pre-hook failed, treating as allow: {e}"
                        );
                    } else {
                        tracing::warn!(
                            hook = hook.command,
                            "pre-hook failed, blocking tool call (fail_open=false): {e}"
                        );
                        return PreToolAction::Deny {
                            message: format!("hook error: {e}"),
                        };
                    }
                }
            }
        }

        if current_args != *args {
            PreToolAction::Modify { args: current_args }
        } else {
            PreToolAction::Allow
        }
    }

    /// Run all matching post-tool-use hooks. Returns the (possibly modified) result string.
    pub async fn run_post_hooks(&self, tool_name: &str, args: &Value, result: &str) -> String {
        if self.config.post_tool_use.is_empty() {
            return result.to_string();
        }

        let mut current_result = result.to_string();

        for hook in &self.config.post_tool_use {
            if !matches_tool(hook, tool_name) {
                continue;
            }

            let input = serde_json::json!({
                "hook": "post_tool_use",
                "tool_name": tool_name,
                "args": args,
                "result": current_result,
            });

            match run_hook_process(&hook.command, &input, hook.timeout_secs).await {
                Ok(stdout) => match serde_json::from_str::<PostToolAction>(&stdout) {
                    Ok(PostToolAction::Modify { result: new_result }) => {
                        current_result = new_result;
                    }
                    Ok(PostToolAction::Passthrough) => {}
                    Err(e) => {
                        tracing::warn!(
                            hook = hook.command,
                            "post-hook returned unparseable output, treating as passthrough: {e}"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        hook = hook.command,
                        "post-hook failed, treating as passthrough: {e}"
                    );
                }
            }
        }

        current_result
    }

    /// Run auto-save before file operations to sync IDE buffers to disk.
    /// Returns Ok(()) if not configured, tool doesn't match, or save succeeded.
    /// Returns Err(message) only when fail_open=false and the command fails.
    pub async fn run_auto_save(&self, tool_name: &str, args: &Value) -> Result<(), String> {
        let auto = match &self.config.auto_save {
            Some(a) => a,
            None => return Ok(()),
        };
        let command = match &auto.command {
            Some(c) if !c.is_empty() => c,
            _ => return Ok(()),
        };

        // Only trigger for file-mutating tools (and optionally read_file)
        let should_trigger = match tool_name {
            "edit_file" | "write_file" => true,
            "read_file" => auto.before_read,
            _ => false,
        };
        if !should_trigger {
            return Ok(());
        }

        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let input = serde_json::json!({
            "hook": "auto_save",
            "tool_name": tool_name,
            "path": path,
        });

        match run_hook_process(command, &input, auto.timeout_secs).await {
            Ok(_) => {
                // Brief pause for filesystem propagation (macOS FSEvents can lag)
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(())
            }
            Err(e) => {
                if auto.fail_open {
                    tracing::warn!(command, "auto_save hook failed, proceeding anyway: {e}");
                    Ok(())
                } else {
                    Err(format!("auto_save failed: {e}"))
                }
            }
        }
    }

    pub fn has_pre_hooks(&self) -> bool {
        !self.config.pre_tool_use.is_empty()
    }

    pub fn has_post_hooks(&self) -> bool {
        !self.config.post_tool_use.is_empty()
    }
}

/// Check if a hook entry's tool_pattern matches the given tool name.
/// Supports glob patterns via `globset` (e.g. `write_*`, `*_file`, `mcp_*_read`).
fn matches_tool(hook: &HookEntry, tool_name: &str) -> bool {
    match &hook.tool_pattern {
        None => true,
        Some(pattern) => globset::Glob::new(pattern)
            .map(|g| g.compile_matcher().is_match(tool_name))
            .unwrap_or(false),
    }
}

/// Spawn a shell process, pipe JSON to stdin, read stdout with timeout.
async fn run_hook_process(
    command: &str,
    input: &Value,
    timeout_secs: u64,
) -> anyhow::Result<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no stdin"))?;
        let payload = serde_json::to_string(input)?;
        stdin.write_all(payload.as_bytes()).await?;
        stdin.shutdown().await?;
    }
    // Drop stdin handle so the child can read EOF
    child.stdin.take();

    // kill_on_drop ensures the child is killed if the timeout fires and the future is dropped
    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("hook timed out after {timeout_secs}s"))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "hook exited with status {}: {}",
            output.status,
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_tool_exact() {
        let hook = HookEntry {
            command: "echo".into(),
            tool_pattern: Some("bash".into()),
            timeout_secs: 5,
            fail_open: true,
        };
        assert!(matches_tool(&hook, "bash"));
        assert!(!matches_tool(&hook, "read_file"));
    }

    #[test]
    fn matches_tool_glob_suffix() {
        let hook = HookEntry {
            command: "echo".into(),
            tool_pattern: Some("write_*".into()),
            timeout_secs: 5,
            fail_open: true,
        };
        assert!(matches_tool(&hook, "write_file"));
        assert!(matches_tool(&hook, "write_memory"));
        assert!(!matches_tool(&hook, "read_file"));
    }

    #[test]
    fn matches_tool_glob_prefix() {
        let hook = HookEntry {
            command: "echo".into(),
            tool_pattern: Some("*_file".into()),
            timeout_secs: 5,
            fail_open: true,
        };
        assert!(matches_tool(&hook, "read_file"));
        assert!(matches_tool(&hook, "write_file"));
        assert!(!matches_tool(&hook, "bash"));
    }

    #[test]
    fn matches_tool_none_matches_all() {
        let hook = HookEntry {
            command: "echo".into(),
            tool_pattern: None,
            timeout_secs: 5,
            fail_open: true,
        };
        assert!(matches_tool(&hook, "anything"));
    }

    #[tokio::test]
    async fn pre_hooks_empty_config_allows() {
        let runner = HookRunner::new(HooksConfig::default());
        let action = runner.run_pre_hooks("bash", &serde_json::json!({})).await;
        assert!(matches!(action, PreToolAction::Allow));
    }

    #[tokio::test]
    async fn post_hooks_empty_config_passes_through() {
        let runner = HookRunner::new(HooksConfig::default());
        let result = runner
            .run_post_hooks("bash", &serde_json::json!({}), "original")
            .await;
        assert_eq!(result, "original");
    }

    #[tokio::test]
    async fn auto_save_not_configured_is_ok() {
        let runner = HookRunner::new(HooksConfig::default());
        let result = runner
            .run_auto_save("edit_file", &serde_json::json!({"path": "/tmp/f.rs"}))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn auto_save_triggers_for_edit_and_write() {
        use config::AutoSaveConfig;
        let config = HooksConfig {
            auto_save: Some(AutoSaveConfig {
                command: Some("echo saved".into()),
                timeout_secs: 3,
                before_read: false,
                fail_open: true,
            }),
            ..Default::default()
        };
        let runner = HookRunner::new(config);
        assert!(
            runner
                .run_auto_save("edit_file", &serde_json::json!({"path": "/tmp/f.rs"}))
                .await
                .is_ok()
        );
        assert!(
            runner
                .run_auto_save("write_file", &serde_json::json!({"path": "/tmp/f.rs"}))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn auto_save_skips_unrelated_tools() {
        use config::AutoSaveConfig;
        let config = HooksConfig {
            auto_save: Some(AutoSaveConfig {
                // Command that would fail if actually run
                command: Some("exit 1".into()),
                timeout_secs: 3,
                before_read: false,
                fail_open: false,
            }),
            ..Default::default()
        };
        let runner = HookRunner::new(config);
        // Non-file tools should skip entirely (return Ok even with fail_open=false)
        assert!(
            runner
                .run_auto_save("list_dir", &serde_json::json!({}))
                .await
                .is_ok()
        );
        assert!(
            runner
                .run_auto_save("bash", &serde_json::json!({}))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn auto_save_read_file_respects_before_read() {
        use config::AutoSaveConfig;
        // before_read=false: read_file should not trigger
        let config = HooksConfig {
            auto_save: Some(AutoSaveConfig {
                command: Some("exit 1".into()),
                timeout_secs: 3,
                before_read: false,
                fail_open: false,
            }),
            ..Default::default()
        };
        let runner = HookRunner::new(config);
        assert!(
            runner
                .run_auto_save("read_file", &serde_json::json!({"path": "/tmp/f.rs"}))
                .await
                .is_ok()
        );

        // before_read=true: read_file should trigger (and fail here)
        let config2 = HooksConfig {
            auto_save: Some(AutoSaveConfig {
                command: Some("exit 1".into()),
                timeout_secs: 3,
                before_read: true,
                fail_open: false,
            }),
            ..Default::default()
        };
        let runner2 = HookRunner::new(config2);
        assert!(
            runner2
                .run_auto_save("read_file", &serde_json::json!({"path": "/tmp/f.rs"}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn auto_save_fail_open_swallows_error() {
        use config::AutoSaveConfig;
        let config = HooksConfig {
            auto_save: Some(AutoSaveConfig {
                command: Some("exit 1".into()),
                timeout_secs: 3,
                before_read: false,
                fail_open: true,
            }),
            ..Default::default()
        };
        let runner = HookRunner::new(config);
        assert!(
            runner
                .run_auto_save("edit_file", &serde_json::json!({"path": "/tmp/f.rs"}))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn auto_save_fail_closed_returns_error() {
        use config::AutoSaveConfig;
        let config = HooksConfig {
            auto_save: Some(AutoSaveConfig {
                command: Some("exit 1".into()),
                timeout_secs: 3,
                before_read: false,
                fail_open: false,
            }),
            ..Default::default()
        };
        let runner = HookRunner::new(config);
        let result = runner
            .run_auto_save("edit_file", &serde_json::json!({"path": "/tmp/f.rs"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("auto_save failed"));
    }
}
