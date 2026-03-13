use std::collections::HashSet;
use std::io::Write;

use crate::approval_bridge::{ApprovalClient, ApprovalDecision, ApprovalRequest};
use crate::common::truncate_with_ellipsis;
use crate::mcp::McpRegistry;
use crate::render::Renderer;
use crate::tools::BuiltinTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PermissionMode {
    /// Prompt for writes; reads auto-allowed (default).
    Default,
    /// Auto-allow file edits/writes; prompt for shell commands.
    #[clap(name = "accept-edits")]
    AcceptEdits,
    /// Read-only exploration; all writes denied.
    Plan,
    /// Auto-allow all tools without prompting.
    #[clap(name = "dont-ask")]
    DontAsk,
    /// Auto-allow all tools and skip write discipline guards.
    #[clap(name = "bypass")]
    BypassPermissions,
    /// Internal: set automatically in non-interactive (no-TTY) mode.
    #[clap(skip)]
    Auto,
}

impl PermissionMode {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "accept-edits",
            Self::Plan => "plan",
            Self::DontAsk => "dont-ask",
            Self::BypassPermissions => "bypass",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny,
}

pub const PERMISSION_DENIED_MSG: &str = "Permission denied by user. Do not retry this tool call. \
     Inform the user what you wanted to do and ask how to proceed.";

/// File-mutation tools: write/edit file content or agent memory — no shell execution.
pub fn is_file_mutation(tool_name: &str) -> bool {
    matches!(
        BuiltinTool::from_str(tool_name),
        Some(BuiltinTool::WriteFile | BuiltinTool::EditFile | BuiltinTool::SaveMemory)
    )
}

/// Shared tool classification: read-only tools that don't mutate state.
pub fn is_read_only(tool_name: &str) -> bool {
    if McpRegistry::is_mcp_tool(tool_name) {
        return true;
    }
    matches!(
        BuiltinTool::from_str(tool_name),
        Some(
            BuiltinTool::ReadFile
                | BuiltinTool::ListDir
                | BuiltinTool::GlobFiles
                | BuiltinTool::GrepFiles
                | BuiltinTool::WebFetch
                | BuiltinTool::AddDir
        )
    )
}

pub struct ToolPermissionGate {
    mode: PermissionMode,
    session_allowed: HashSet<String>,
    interactive: bool,
    renderer: Renderer,
    bridge_client: Option<ApprovalClient>,
    /// True when a plan file is configured — enables structural enforcement where
    /// read-only tools are auto-allowed and writes are guarded by context guardrails.
    plan_file_set: bool,
}

impl ToolPermissionGate {
    pub fn new(mode: PermissionMode, interactive: bool) -> Self {
        Self {
            mode,
            session_allowed: HashSet::new(),
            interactive,
            renderer: Renderer::new(),
            bridge_client: None,
            plan_file_set: false,
        }
    }

    pub fn set_plan_file_enforcement(&mut self, enabled: bool) {
        self.plan_file_set = enabled;
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    /// Switch permission mode mid-session (e.g. from a /mode REPL command).
    pub fn set_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    /// Returns true when write discipline guards (Guard A/0/B/C, staleness) should be skipped.
    /// Only BypassPermissions opts out of structural guardrails.
    pub fn skip_write_guards(&self) -> bool {
        self.mode == PermissionMode::BypassPermissions
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    /// Resolve the effective permission mode: explicit > heuristic (tty check).
    /// Tries to connect to the approval bridge (Zed) for interactive modes.
    pub fn resolve(explicit: Option<PermissionMode>) -> Self {
        let mut gate = match explicit {
            Some(mode) => {
                let interactive = mode != PermissionMode::Auto && tty_available();
                Self::new(mode, interactive)
            }
            None => {
                if tty_available() {
                    Self::new(PermissionMode::Default, true)
                } else {
                    Self::new(PermissionMode::Auto, false)
                }
            }
        };

        // Try connecting to the Zed approval bridge — only useful when prompting may occur
        let needs_bridge = !matches!(
            gate.mode,
            PermissionMode::Auto | PermissionMode::DontAsk | PermissionMode::BypassPermissions
        );
        if needs_bridge {
            let cwd = std::env::current_dir().unwrap_or_default();
            gate.bridge_client = ApprovalClient::try_connect(&cwd);
            if gate.bridge_client.is_some() {
                tracing::info!("connected to Zed approval bridge");
            }
        }

        gate
    }

    pub fn check_permission(
        &mut self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> PermissionDecision {
        match self.mode {
            // Auto/DontAsk/BypassPermissions: allow everything without prompting
            PermissionMode::Auto | PermissionMode::DontAsk | PermissionMode::BypassPermissions => {
                PermissionDecision::Allow
            }
            // AcceptEdits: auto-allow reads and file mutations; prompt for shell/other tools
            PermissionMode::AcceptEdits => {
                if is_read_only(tool_name) || is_file_mutation(tool_name) {
                    PermissionDecision::Allow
                } else {
                    self.prompt_or_allow(tool_name, args)
                }
            }
            PermissionMode::Default => {
                if is_read_only(tool_name) {
                    return PermissionDecision::Allow;
                }
                self.prompt_or_allow(tool_name, args)
            }
            PermissionMode::Plan => {
                // Reads always allowed in plan mode — the whole point is exploration
                if is_read_only(tool_name) {
                    return PermissionDecision::Allow;
                }
                // With plan file: writes go through prompt+guardrail chain (restricted to plan file)
                // Without plan file: hard-deny all writes (no prompt, no override)
                if self.plan_file_set {
                    self.prompt_or_allow(tool_name, args)
                } else {
                    PermissionDecision::Deny
                }
            }
        }
    }

    fn prompt_or_allow(&mut self, tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        if self.session_allowed.contains(tool_name) {
            return PermissionDecision::Allow;
        }
        if !self.interactive {
            // Non-interactive but not auto mode — deny by default
            return PermissionDecision::Deny;
        }
        self.prompt_user(tool_name, args)
    }

    fn prompt_user(&mut self, tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        let preview = tool_preview(tool_name, args);

        // Try the Zed approval bridge first
        if let Some(ref mut client) = self.bridge_client {
            let req = ApprovalRequest {
                tool_name: tool_name.to_string(),
                args: args.clone(),
                title: preview.clone(),
            };
            if let Some(resp) = client.request_approval(&req) {
                return match resp.decision {
                    ApprovalDecision::AllowOnce => PermissionDecision::Allow,
                    ApprovalDecision::AllowSession => {
                        self.session_allowed.insert(tool_name.to_string());
                        PermissionDecision::Allow
                    }
                    ApprovalDecision::Deny => PermissionDecision::Deny,
                };
            }
            // Bridge failed — Zed probably crashed. Clear it and fall through to TTY.
            tracing::warn!("approval bridge disconnected, falling back to TTY prompt");
            self.bridge_client = None;
        }

        let diff_section = compute_preview_diff(tool_name, args);

        let border_len = 40.max(tool_name.len() + 4);
        let top = format!(
            "┌ {} {}",
            tool_name,
            "─".repeat(border_len - tool_name.len() - 3)
        );
        let bottom = format!("└{}", "─".repeat(border_len - 1));

        eprint!("\n{top}\n│ {preview}");
        if let Some((path, old, new)) = diff_section {
            let diff = self.renderer.render_diff(&path, &old, &new);
            for line in diff.lines() {
                eprint!("\n│ {line}");
            }
        }
        eprint!("\n{bottom}\n  [y] Allow once  [a] Allow for session  [n] Deny: ");
        let _ = std::io::stderr().flush();

        let response = read_tty_char();
        eprintln!();

        match response {
            Some('y') | Some('Y') => PermissionDecision::Allow,
            Some('a') | Some('A') => {
                self.session_allowed.insert(tool_name.to_string());
                PermissionDecision::Allow
            }
            _ => PermissionDecision::Deny,
        }
    }
}

fn tool_preview(tool_name: &str, args: &serde_json::Value) -> String {
    match BuiltinTool::from_str(tool_name) {
        Some(BuiltinTool::Bash | BuiltinTool::RunCommand) => {
            truncate_with_ellipsis(args["command"].as_str().unwrap_or("(unknown)"), 120)
        }
        Some(BuiltinTool::WriteFile) => {
            let path = args["path"].as_str().unwrap_or("(unknown)");
            let content_len = args["content"].as_str().map(|s| s.len()).unwrap_or(0);
            format!("{path} ({content_len} bytes)")
        }
        Some(BuiltinTool::EditFile) => args["path"].as_str().unwrap_or("(unknown)").to_string(),
        Some(BuiltinTool::SaveMemory) => {
            let key = args["key"].as_str().unwrap_or("note");
            format!("key={key}")
        }
        Some(BuiltinTool::SpawnAgent) => {
            truncate_with_ellipsis(args["task"].as_str().unwrap_or("(unknown)"), 100)
        }
        _ => truncate_with_ellipsis(&args.to_string(), 120),
    }
}

/// Compute a preview diff for edit/write tools by reading the current file content.
/// Returns (path, old_content, new_content) if applicable.
fn compute_preview_diff(
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<(String, String, String)> {
    match BuiltinTool::from_str(tool_name) {
        Some(BuiltinTool::EditFile) => {
            let path = args["path"].as_str()?;
            let old_string = args["old_string"].as_str()?;
            let new_string = args["new_string"].as_str()?;
            let contents = std::fs::read_to_string(path).ok()?;
            if contents.matches(old_string).count() != 1 {
                return None;
            }
            let new_contents = contents.replacen(old_string, new_string, 1);
            Some((path.to_string(), contents, new_contents))
        }
        Some(BuiltinTool::WriteFile) => {
            let path = args["path"].as_str()?;
            let new_content = args["content"].as_str().unwrap_or("");
            let old_content = std::fs::read_to_string(path).unwrap_or_default();
            Some((path.to_string(), old_content, new_content.to_string()))
        }
        _ => None,
    }
}

/// Read a single character from /dev/tty.
fn read_tty_char() -> Option<char> {
    use std::io::Read;
    let mut tty = std::fs::File::open("/dev/tty").ok()?;
    let mut buf = [0u8; 4];
    let n = tty.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    std::str::from_utf8(&buf[..n])
        .ok()
        .and_then(|s| s.trim().chars().next())
}

fn tty_available() -> bool {
    std::fs::File::open("/dev/tty").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auto_mode_always_allows() {
        let mut gate = ToolPermissionGate::new(PermissionMode::Auto, false);
        assert_eq!(
            gate.check_permission("bash", &json!({"command": "rm -rf /"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("read_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn default_mode_allows_read_tools() {
        let mut gate = ToolPermissionGate::new(PermissionMode::Default, false);
        assert_eq!(
            gate.check_permission("read_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("glob_files", &json!({"pattern": "*.rs"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("list_dir", &json!({"path": "."})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("grep_files", &json!({"pattern": "foo"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("web_fetch", &json!({"url": "http://example.com"})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn default_mode_denies_write_tools_non_interactive() {
        let mut gate = ToolPermissionGate::new(PermissionMode::Default, false);
        // Non-interactive + non-auto → deny write tools
        assert_eq!(
            gate.check_permission("bash", &json!({"command": "ls"})),
            PermissionDecision::Deny
        );
        assert_eq!(
            gate.check_permission("write_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn plan_mode_allows_reads_denies_writes_without_plan_file() {
        let mut gate = ToolPermissionGate::new(PermissionMode::Plan, false);
        // Reads always allowed in plan mode, even without plan file
        assert_eq!(
            gate.check_permission("read_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("glob_files", &json!({"pattern": "*.rs"})),
            PermissionDecision::Allow
        );
        // Writes hard-denied without plan file
        assert_eq!(
            gate.check_permission("bash", &json!({"command": "ls"})),
            PermissionDecision::Deny
        );
        assert_eq!(
            gate.check_permission("write_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn plan_mode_hard_denies_writes_interactive_no_plan_file() {
        // Even with interactive=true, writes are hard-denied without plan file
        let mut gate = ToolPermissionGate::new(PermissionMode::Plan, true);
        assert_eq!(
            gate.check_permission("read_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("bash", &json!({"command": "rm -rf /"})),
            PermissionDecision::Deny
        );
        assert_eq!(
            gate.check_permission("write_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn structural_plan_mode_auto_allows_reads() {
        // When plan_file_set is true, read-only tools bypass the prompt in plan mode.
        let mut gate = ToolPermissionGate::new(PermissionMode::Plan, false);
        gate.set_plan_file_enforcement(true);
        assert_eq!(
            gate.check_permission("read_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow,
            "reads auto-allowed in structural plan mode"
        );
        assert_eq!(
            gate.check_permission("glob_files", &json!({"pattern": "*.rs"})),
            PermissionDecision::Allow,
        );
        // Non-interactive → write tools still denied (guardrails handle the enforcement)
        assert_eq!(
            gate.check_permission("write_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Deny,
            "writes still require prompt (guardrails enforce file restriction)"
        );
        // bash is denied (not read-only, not auto-allowed)
        assert_eq!(
            gate.check_permission("bash", &json!({"command": "ls"})),
            PermissionDecision::Deny,
        );
    }

    #[test]
    fn session_allowed_skips_prompt() {
        let mut gate = ToolPermissionGate::new(PermissionMode::Default, false);
        gate.session_allowed.insert("bash".to_string());
        assert_eq!(
            gate.check_permission("bash", &json!({"command": "ls"})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn is_read_only_classification() {
        assert!(is_read_only("read_file"));
        assert!(is_read_only("list_dir"));
        assert!(is_read_only("glob_files"));
        assert!(is_read_only("grep_files"));
        assert!(is_read_only("web_fetch"));
        assert!(is_read_only("add_dir"));
        assert!(!is_read_only("bash"));
        assert!(!is_read_only("write_file"));
        assert!(!is_read_only("edit_file"));
        assert!(!is_read_only("run_command"));
        assert!(!is_read_only("save_memory"));
        assert!(!is_read_only("spawn_agent"));
    }

    #[test]
    fn permission_mode_parsing() {
        use clap::ValueEnum;
        assert_eq!(
            PermissionMode::from_str("default", true),
            Ok(PermissionMode::Default)
        );
        assert_eq!(
            PermissionMode::from_str("plan", true),
            Ok(PermissionMode::Plan)
        );
        // "auto" is #[clap(skip)] — not parseable from CLI
        assert!(PermissionMode::from_str("auto", true).is_err());
        assert!(PermissionMode::from_str("invalid", true).is_err());
    }

    #[test]
    fn permission_mode_parsing_new_variants() {
        use clap::ValueEnum;
        assert_eq!(
            PermissionMode::from_str("accept-edits", true),
            Ok(PermissionMode::AcceptEdits)
        );
        assert_eq!(
            PermissionMode::from_str("dont-ask", true),
            Ok(PermissionMode::DontAsk)
        );
        assert_eq!(
            PermissionMode::from_str("bypass", true),
            Ok(PermissionMode::BypassPermissions)
        );
    }

    #[test]
    fn accept_edits_allows_file_mutations() {
        let mut gate = ToolPermissionGate::new(PermissionMode::AcceptEdits, false);
        // File mutations auto-allowed
        assert_eq!(
            gate.check_permission("write_file", &json!({"path": "foo.rs", "content": ""})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("edit_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("save_memory", &json!({"key": "x", "value": "y"})),
            PermissionDecision::Allow
        );
        // Reads auto-allowed
        assert_eq!(
            gate.check_permission("read_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn accept_edits_denies_shell_non_interactive() {
        let mut gate = ToolPermissionGate::new(PermissionMode::AcceptEdits, false);
        // Shell tools require prompt; non-interactive → deny
        assert_eq!(
            gate.check_permission("bash", &json!({"command": "ls"})),
            PermissionDecision::Deny
        );
        assert_eq!(
            gate.check_permission("run_command", &json!({"command": "ls"})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn dont_ask_allows_everything() {
        let mut gate = ToolPermissionGate::new(PermissionMode::DontAsk, false);
        assert_eq!(
            gate.check_permission("bash", &json!({"command": "rm -rf /"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("write_file", &json!({"path": "foo.rs", "content": ""})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("read_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn bypass_allows_everything() {
        let mut gate = ToolPermissionGate::new(PermissionMode::BypassPermissions, false);
        assert_eq!(
            gate.check_permission("bash", &json!({"command": "rm -rf /"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            gate.check_permission("write_file", &json!({"path": "foo.rs", "content": ""})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn bypass_skip_write_guards() {
        let gate = ToolPermissionGate::new(PermissionMode::BypassPermissions, false);
        assert!(gate.skip_write_guards());

        let gate2 = ToolPermissionGate::new(PermissionMode::DontAsk, false);
        assert!(!gate2.skip_write_guards());

        let gate3 = ToolPermissionGate::new(PermissionMode::AcceptEdits, false);
        assert!(!gate3.skip_write_guards());
    }

    #[test]
    fn is_file_mutation_classification() {
        assert!(is_file_mutation("write_file"));
        assert!(is_file_mutation("edit_file"));
        assert!(is_file_mutation("save_memory"));
        assert!(!is_file_mutation("bash"));
        assert!(!is_file_mutation("run_command"));
        assert!(!is_file_mutation("read_file"));
        assert!(!is_file_mutation("glob_files"));
        assert!(!is_file_mutation("spawn_agent"));
    }

    #[test]
    fn set_mode_switches_behavior() {
        let mut gate = ToolPermissionGate::new(PermissionMode::Plan, false);
        // Plan mode denies writes
        assert_eq!(
            gate.check_permission("write_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Deny
        );
        // Switch to DontAsk — now allows everything
        gate.set_mode(PermissionMode::DontAsk);
        assert_eq!(
            gate.check_permission("write_file", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn compute_preview_diff_edit() {
        use std::io::Write as _;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "fn main() {{\n    println!(\"hello\");\n}}\n").unwrap();
        let path = f.path().to_str().unwrap();

        let args = json!({
            "path": path,
            "old_string": "println!(\"hello\")",
            "new_string": "println!(\"world\")"
        });
        let result = compute_preview_diff("edit_file", &args);
        assert!(result.is_some());
        let (p, old, new) = result.unwrap();
        assert_eq!(p, path);
        assert!(old.contains("hello"));
        assert!(new.contains("world"));
    }

    #[test]
    fn compute_preview_diff_write_new_file() {
        let args = json!({
            "path": "/tmp/nonexistent_prism_test_file_xyz.rs",
            "content": "fn main() {}"
        });
        let result = compute_preview_diff("write_file", &args);
        assert!(result.is_some());
        let (_, old, new) = result.unwrap();
        assert!(old.is_empty());
        assert_eq!(new, "fn main() {}");
    }

    #[test]
    fn compute_preview_diff_unrelated_tool() {
        let args = json!({"command": "ls"});
        assert!(compute_preview_diff("bash", &args).is_none());
    }

    #[test]
    fn bridge_fallback_to_tty() {
        // When no socket exists, bridge_client should be None after resolve
        let gate = ToolPermissionGate::resolve(Some(PermissionMode::Default));
        assert!(gate.bridge_client.is_none());
    }
}
