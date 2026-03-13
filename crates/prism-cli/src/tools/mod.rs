pub mod computer;
mod files;
mod search;
mod shell;
mod web;

use std::path::{Path, PathBuf};

use crate::config::{Config, SandboxMode};
use prism_types::{Tool, ToolFunction};
use serde_json::json;

use crate::mcp::McpRegistry;

/// Result type for tool dispatch — text or multimodal image content.
pub enum ToolResult {
    Text(String),
    Multimodal(serde_json::Value),
}

impl ToolResult {
    pub fn into_text(self) -> String {
        match self {
            ToolResult::Text(s) => s,
            ToolResult::Multimodal(v) => v.to_string(),
        }
    }
}

/// Write-capable tool names (blocked in ReadOnly mode).
const WRITE_TOOLS: &[&str] = &["write_file", "edit_file", "bash", "run_command"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinTool {
    ReadFile,
    WriteFile,
    EditFile,
    ListDir,
    Bash,
    RunCommand,
    GlobFiles,
    GrepFiles,
    WebFetch,
    SaveMemory,
    SpawnAgent,
    Recall,
    RecordDecision,
    Skill,
    CheckBackgroundTasks,
    AddDir,
    AskHuman,
    ReportBlocker,
    ReportFinding,
}

impl BuiltinTool {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "read_file" => Some(Self::ReadFile),
            "write_file" => Some(Self::WriteFile),
            "edit_file" => Some(Self::EditFile),
            "list_dir" => Some(Self::ListDir),
            "bash" => Some(Self::Bash),
            "run_command" => Some(Self::RunCommand),
            "glob_files" => Some(Self::GlobFiles),
            "grep_files" => Some(Self::GrepFiles),
            "web_fetch" => Some(Self::WebFetch),
            "save_memory" => Some(Self::SaveMemory),
            "spawn_agent" => Some(Self::SpawnAgent),
            "recall" => Some(Self::Recall),
            "record_decision" => Some(Self::RecordDecision),
            "skill" => Some(Self::Skill),
            "check_background_tasks" => Some(Self::CheckBackgroundTasks),
            "add_dir" => Some(Self::AddDir),
            "ask_human" => Some(Self::AskHuman),
            "report_blocker" => Some(Self::ReportBlocker),
            "report_finding" => Some(Self::ReportFinding),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadFile => "read_file",
            Self::WriteFile => "write_file",
            Self::EditFile => "edit_file",
            Self::ListDir => "list_dir",
            Self::Bash => "bash",
            Self::RunCommand => "run_command",
            Self::GlobFiles => "glob_files",
            Self::GrepFiles => "grep_files",
            Self::WebFetch => "web_fetch",
            Self::SaveMemory => "save_memory",
            Self::SpawnAgent => "spawn_agent",
            Self::Recall => "recall",
            Self::RecordDecision => "record_decision",
            Self::Skill => "skill",
            Self::CheckBackgroundTasks => "check_background_tasks",
            Self::AddDir => "add_dir",
            Self::AskHuman => "ask_human",
            Self::ReportBlocker => "report_blocker",
            Self::ReportFinding => "report_finding",
        }
    }

    /// All tools that are not shell-execution tools (Bash, RunCommand).
    /// Used by plan-mode guardrails to enumerate the permitted tool set.
    pub fn all_non_shell() -> &'static [BuiltinTool] {
        &[
            Self::ReadFile,
            Self::WriteFile,
            Self::EditFile,
            Self::ListDir,
            Self::GlobFiles,
            Self::GrepFiles,
            Self::WebFetch,
            Self::SaveMemory,
            Self::SpawnAgent,
            Self::Recall,
            Self::RecordDecision,
            Self::Skill,
            Self::CheckBackgroundTasks,
            Self::AddDir,
            Self::AskHuman,
            Self::ReportBlocker,
            Self::ReportFinding,
        ]
    }
}

pub fn tool_definitions() -> Vec<Tool> {
    vec![
        make_tool(
            "read_file",
            "Read file contents. Use offset (1-based line number) and limit to read a section of a large file.",
            json!({ "type": "object", "properties": {
                "path":   { "type": "string" },
                "offset": { "type": "integer", "description": "1-based line number to start reading from" },
                "limit":  { "type": "integer", "description": "maximum number of lines to read" }
            }, "required": ["path"] }),
        ),
        make_tool(
            "write_file",
            "Write content to a file (creates parent dirs)",
            json!({ "type": "object", "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            }, "required": ["path", "content"] }),
        ),
        make_tool(
            "list_dir",
            "List directory entries",
            json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        ),
        make_tool(
            "run_command",
            "Run a shell command; returns stdout/stderr/exit_code",
            json!({ "type": "object", "properties": {
                "command":      { "type": "string" },
                "args":         { "type": "array", "items": { "type": "string" } },
                "timeout_secs": { "type": "integer" },
                "cwd":          { "type": "string", "description": "working directory for the command (default: current dir)" }
            }, "required": ["command"] }),
        ),
        make_tool(
            "bash",
            "Run a shell command string via sh -c. Returns {exit_code, stdout, stderr}. Default timeout 30s (max 120s).",
            json!({ "type": "object", "properties": {
                "command":      { "type": "string", "description": "shell command to run via sh -c" },
                "timeout_secs": { "type": "integer" },
                "cwd":          { "type": "string", "description": "working directory for the command (default: current dir)" }
            }, "required": ["command"] }),
        ),
        make_tool(
            "edit_file",
            "Replace an exact string in a file. Fails if old_string is not found or appears more than once — add more surrounding context to make it unique.",
            json!({ "type": "object", "properties": {
                "path":       { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" }
            }, "required": ["path", "old_string", "new_string"] }),
        ),
        make_tool(
            "glob_files",
            "Find files by glob pattern (e.g. '**/*.rs'). Returns array of matching paths. Use sort_by='modified' to sort newest-first by mtime.",
            json!({ "type": "object", "properties": {
                "pattern":     { "type": "string" },
                "dir":         { "type": "string", "description": "root dir to search (default '.')" },
                "max_results": { "type": "integer" },
                "sort_by":     { "type": "string", "description": "sort order: 'modified' for newest-first by mtime" }
            }, "required": ["pattern"] }),
        ),
        make_tool(
            "grep_files",
            "Search file contents by regex. output_mode: 'content' (default) returns [{path, line, text}], 'files' returns [path], 'count' returns [{path, count}]. Use context for surrounding lines in content mode.",
            json!({ "type": "object", "properties": {
                "pattern":     { "type": "string", "description": "regex pattern" },
                "dir":         { "type": "string", "description": "root dir (default '.')" },
                "file_glob":   { "type": "string", "description": "optional glob to filter files, e.g. '*.rs'" },
                "max_results": { "type": "integer" },
                "output_mode": { "type": "string", "description": "output mode: 'content' (default), 'files', or 'count'" },
                "context":     { "type": "integer", "description": "lines of context before/after each match (content mode only)" }
            }, "required": ["pattern"] }),
        ),
        make_tool(
            "web_fetch",
            "Fetch a URL and return its text content (HTML tags stripped). Use for documentation, APIs, or any publicly accessible page.",
            json!({ "type": "object", "properties": {
                "url": { "type": "string", "description": "fully qualified URL to fetch" }
            }, "required": ["url"] }),
        ),
        make_tool(
            "save_memory",
            "Save a fact to persistent memory. The memory persists across sessions and is injected into your system prompt.",
            json!({ "type": "object", "properties": {
                "key":   { "type": "string", "description": "Short label for this memory (e.g. 'project_structure', 'user_preference')" },
                "value": { "type": "string", "description": "The content to remember" }
            }, "required": ["key", "value"] }),
        ),
        make_tool(
            "spawn_agent",
            "Spawn a sub-agent to execute a task. The sub-agent runs independently and returns a JSON result with status, summary, and cost. Set run_in_background=true to fire-and-forget. Use thread to assign the child to a context thread, and constraints to restrict file/tool access or set cost caps.",
            json!({ "type": "object", "properties": {
                "task":              { "type": "string", "description": "Natural language task for the sub-agent" },
                "model":             { "type": "string", "description": "Model to use (optional, defaults to parent model)" },
                "cost_cap":          { "type": "number", "description": "Max cost in USD (optional)" },
                "timeout_secs":      { "type": "integer", "description": "Timeout in seconds (default 300)" },
                "run_in_background": { "type": "boolean", "description": "If true, run in background and return immediately. You'll be notified when it completes." },
                "thread":            { "type": "string", "description": "Thread name to assign the child agent to" },
                "constraints":       { "type": "object", "description": "Handoff constraints: {cost_cap, timeout_secs, allowed_tools, allowed_files}",
                    "properties": {
                        "cost_cap":      { "type": "number" },
                        "timeout_secs":  { "type": "integer" },
                        "allowed_tools": { "type": "array", "items": { "type": "string" } },
                        "allowed_files": { "type": "array", "items": { "type": "string" } }
                    }
                },
                "handoff_mode":      { "type": "string", "description": "delegate_and_await (default) or delegate_and_forget" }
            }, "required": ["task"] }),
        ),
        make_tool(
            "recall",
            "Load context from the uglyhat context store. Recall a thread by name or search by tags. Returns memories, decisions, and recent activity.",
            json!({ "type": "object", "properties": {
                "thread": { "type": "string", "description": "Thread name to recall (e.g. 'auth-refactor')" },
                "tags":   { "type": "array", "items": { "type": "string" }, "description": "Tags to search for (returns matching memories + decisions)" },
                "since":  { "type": "string", "description": "Duration like '2h', '30m', '1d' — returns everything since that time" }
            } }),
        ),
        make_tool(
            "record_decision",
            "Record an architectural or implementation decision with rationale. Persists across sessions and is visible to other agents.",
            json!({ "type": "object", "properties": {
                "title":   { "type": "string", "description": "Short decision label (e.g. 'Use local table for rate limits')" },
                "content": { "type": "string", "description": "Rationale — why this was chosen over alternatives" },
                "thread":  { "type": "string", "description": "Thread name to associate with (defaults to current thread)" },
                "tags":    { "type": "array", "items": { "type": "string" }, "description": "Tags for filtering" },
                "scope":   { "type": "string", "description": "'thread' (default) or 'workspace' — workspace-scoped decisions notify all agents" }
            }, "required": ["title", "content"] }),
        ),
        make_tool(
            "skill",
            "Execute a skill by name. Skills are specialized prompt templates discovered from .prism/skills/ directories.",
            json!({ "type": "object", "properties": {
                "name": { "type": "string", "description": "Skill name to execute (e.g. 'commit', 'review-pr')" },
                "args": { "type": "string", "description": "Optional arguments to pass to the skill" }
            }, "required": ["name"] }),
        ),
        make_tool(
            "check_background_tasks",
            "Check the status of background tasks. Returns active (still running) and newly completed tasks.",
            json!({ "type": "object", "properties": {} }),
        ),
        make_tool(
            "add_dir",
            "Add a directory to the working context. Returns a listing of the directory contents. Use this when you need to work with files outside the primary working directory.",
            json!({ "type": "object", "properties": {
                "path": { "type": "string", "description": "Absolute path to the directory to add" }
            }, "required": ["path"] }),
        ),
        make_tool(
            "ask_human",
            "Post a question or request to the human operator's inbox. Use when you need clarification, approval, or input that only a human can provide. The human will see it at their next REPL prompt.",
            json!({ "type": "object", "properties": {
                "question": { "type": "string", "description": "The question or request to send to the human" },
                "severity": { "type": "string", "enum": ["critical", "warning", "info"], "description": "Urgency level (default: info)" }
            }, "required": ["question"] }),
        ),
        make_tool(
            "report_blocker",
            "Report a validated blocker. You MUST complete the Claim Validation Protocol before calling this: (1) trace initialization, (2) check reachability, (3) check alternative handlers. All evidence fields are required.",
            json!({ "type": "object", "properties": {
                "title": { "type": "string", "description": "Short blocker summary" },
                "description": { "type": "string", "description": "What is blocked and why" },
                "initialization_trace": { "type": "string", "description": "Where the suspect value/condition originates — file paths, line numbers, assignment chain" },
                "reachability": { "type": "string", "description": "Whether this condition is reachable in prod/staging — code path, feature flags, config gates" },
                "alternative_handlers": { "type": "string", "description": "Other handlers that already cover this condition — or what you searched and why none exist" },
                "severity": { "type": "string", "enum": ["critical", "warning"], "description": "critical = work cannot proceed; warning = degraded (default: warning)" },
                "thread": { "type": "string", "description": "Thread name (defaults to current thread)" }
            }, "required": ["title", "description", "initialization_trace", "reachability", "alternative_handlers"] }),
        ),
        make_tool(
            "report_finding",
            "Report a code review finding with a confidence level. Lower confidence levels require less evidence. Use report_blocker for legacy compatibility; prefer report_finding for new review findings.",
            json!({ "type": "object", "properties": {
                "confidence": { "type": "string", "enum": ["blocker", "likely_blocker", "concern", "nit"], "description": "blocker: fully validated, critical stop; likely_blocker: strong evidence but not fully proven; concern: worth addressing, not a hard stop; nit: minor style/quality suggestion" },
                "title": { "type": "string", "description": "Short finding summary" },
                "description": { "type": "string", "description": "What the finding is and why it matters" },
                "initialization_trace": { "type": "string", "description": "Required for blocker and likely_blocker: where the suspect value/condition originates — file paths, line numbers, assignment chain" },
                "reachability": { "type": "string", "description": "Required for blocker and likely_blocker: whether this condition is reachable in prod/staging" },
                "alternative_handlers": { "type": "string", "description": "Required for blocker only: other handlers that already cover this condition, or what you searched and why none exist" },
                "thread": { "type": "string", "description": "Thread name (defaults to current thread)" }
            }, "required": ["confidence", "title", "description"] }),
        ),
    ]
}

/// Filter tool list based on sandbox mode and allow/deny configuration.
pub fn tool_definitions_filtered(config: &Config) -> Vec<Tool> {
    tool_definitions()
        .into_iter()
        .chain(computer::computer_tool_definitions())
        .filter(|t| is_tool_allowed(&t.function.name, config))
        .collect()
}

/// Check whether a named tool may be called under the current config.
pub fn is_tool_allowed(name: &str, config: &Config) -> bool {
    // ReadOnly mode: block write tools
    if config.session.sandbox_mode == SandboxMode::ReadOnly && WRITE_TOOLS.contains(&name) {
        return false;
    }
    // Restricted mode: enforce allowlist — None means deny all (strictest interpretation)
    if config.session.sandbox_mode == SandboxMode::Restricted {
        match &config.session.allowed_tools {
            Some(allowed) => {
                if !allowed.iter().any(|a| a == name) {
                    return false;
                }
            }
            None => return false,
        }
    }
    // Deny list always applies
    if config.session.denied_tools.iter().any(|d| d == name) {
        return false;
    }
    true
}

/// Check whether a file path is denied by the config.
pub fn is_path_denied(path: &str, config: &Config) -> bool {
    use globset::Glob;
    let path_norm = shellexpand::tilde(path).into_owned();
    for pattern in &config.session.denied_paths {
        let pat_norm = shellexpand::tilde(pattern).into_owned();
        if let Ok(glob) = Glob::new(&pat_norm) {
            let matcher = glob.compile_matcher();
            if matcher.is_match(&path_norm) || matcher.is_match(path) {
                return true;
            }
        }
    }
    false
}

/// Check whether a shell command is denied by the config.
///
/// Splits on shell operators (`&&`, `||`, `;`, `|`) and checks each segment so
/// that chained or piped commands cannot bypass a prefix-only check.  Also
/// strips leading `VAR=val` env assignments before matching, which prevents
/// bypasses like `FOO=bar rm -rf /`.
pub fn is_command_denied(cmd: &str, config: &Config) -> bool {
    let cmd_trimmed = cmd.trim();
    // Check the full command string first
    if check_single_command(cmd_trimmed, &config.session.denied_commands) {
        return true;
    }
    // Split on shell operators and check each resulting segment
    for segment in cmd_trimmed.split(&['&', '|', ';'][..]) {
        let seg = segment.trim().trim_start_matches('(').trim();
        // Strip leading env assignments (FOO=bar cmd) before checking
        let effective = seg
            .split_whitespace()
            .skip_while(|w| w.contains('=') && !w.starts_with('-'))
            .collect::<Vec<_>>()
            .join(" ");
        if check_single_command(&effective, &config.session.denied_commands) {
            return true;
        }
    }
    false
}

fn check_single_command(cmd: &str, denied: &[String]) -> bool {
    let cmd_trimmed = cmd.trim();
    for d in denied {
        let d_trimmed = d.trim();
        if cmd_trimmed.starts_with(d_trimmed) {
            // Require a word boundary after the denied prefix (space, tab, or exact match)
            let rest = &cmd_trimmed[d_trimmed.len()..];
            if rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace()) {
                return true;
            }
        }
    }
    false
}

/// Resolve a shell tool's `cwd` argument. Returns `None` when no cwd is available
/// (inherits the process working directory), avoiding a behavioral change for the
/// non-ACP agent path where `session_cwd` is `None`.
fn resolve_shell_cwd(explicit: Option<&str>, session_cwd: Option<&Path>) -> Option<String> {
    let explicit = explicit.filter(|s| !s.is_empty());
    match (explicit, session_cwd) {
        (Some(_), _) => Some(resolve_path(explicit, session_cwd)),
        (None, Some(cwd)) => Some(cwd.to_string_lossy().into_owned()),
        (None, None) => None,
    }
}

/// Resolve a path argument against a session working directory.
/// Returns the path as-is if absolute or no cwd is provided; joins relative paths onto cwd.
fn resolve_path(path: Option<&str>, session_cwd: Option<&Path>) -> String {
    match (path, session_cwd) {
        // Explicit absolute path — use as-is
        (Some(p), _) if !p.is_empty() && Path::new(p).is_absolute() => p.to_string(),
        // Relative path with a session cwd — resolve against it
        (Some(p), Some(cwd)) if !p.is_empty() => cwd.join(p).to_string_lossy().into_owned(),
        // No path arg but we have a session cwd — use cwd as the default
        (None, Some(cwd)) => cwd.to_string_lossy().into_owned(),
        // Fallback: use the provided path or "."
        (Some(p), _) if !p.is_empty() => p.to_string(),
        _ => ".".to_string(),
    }
}

/// Returns built-in tools merged with MCP tools (if any).
pub fn all_tool_definitions(mcp: Option<&McpRegistry>) -> Vec<Tool> {
    let mut tools = tool_definitions();
    if let Some(registry) = mcp {
        tools.extend_from_slice(registry.tool_definitions());
    }
    tools
}

/// Returns built-in + MCP tools, filtered by sandbox mode and allow/deny config.
pub fn all_tool_definitions_filtered(config: &Config, mcp: Option<&McpRegistry>) -> Vec<Tool> {
    all_tool_definitions(mcp)
        .into_iter()
        .chain(computer::computer_tool_definitions())
        .filter(|t| is_tool_allowed(&t.function.name, config))
        .collect()
}

/// Try to resolve a relative path against each additional workspace directory.
/// Returns the first existing, non-denied absolute path, or None.
fn resolve_path_with_fallback(
    relative_path: &str,
    additional_dirs: &[PathBuf],
    config: &Config,
) -> Option<String> {
    for dir in additional_dirs {
        let candidate = dir.join(relative_path);
        if candidate.exists() {
            let s = candidate.to_string_lossy().into_owned();
            if !is_path_denied(&s, config) {
                return Some(s);
            }
        }
    }
    None
}

fn format_dir_list(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Append a hint about additional directories when a file is not found.
fn format_not_found_hint(error: &str, additional_dirs: &[PathBuf]) -> String {
    if additional_dirs.is_empty() {
        return error.to_string();
    }
    format!(
        "{error}\nHint: Try using an absolute path, or search in these additional directories: {}",
        format_dir_list(additional_dirs)
    )
}

/// Append a hint about additional directories when a search returns empty results.
fn format_empty_search_hint(result: &str, additional_dirs: &[PathBuf]) -> String {
    if additional_dirs.is_empty() || result != "[]" {
        return result.to_string();
    }
    format!(
        "[]\nHint: You may also want to search in these additional directories: {}",
        format_dir_list(additional_dirs)
    )
}

pub async fn dispatch(
    name: &str,
    args: &serde_json::Value,
    config: &Config,
    session_cwd: Option<&Path>,
    mcp: Option<&McpRegistry>,
    additional_dirs: &[PathBuf],
) -> ToolResult {
    // Route MCP-namespaced tools to the registry
    if McpRegistry::is_mcp_tool(name) {
        if let Some(registry) = mcp {
            return match registry.dispatch(name, args).await {
                Ok(result) => ToolResult::Text(result),
                Err(e) => ToolResult::Text(format!("{{\"error\": \"{e}\"}}")),
            };
        }
        return ToolResult::Text(format!(
            "{{\"error\": \"MCP tool '{name}' called but no MCP registry available\"}}"
        ));
    }

    // Permission check
    if !is_tool_allowed(name, config) {
        return ToolResult::Text(format!(
            "{{\"error\": \"tool '{name}' is not permitted in current sandbox mode\"}}"
        ));
    }

    // Path-based permission checks — always use the resolved (absolute) path so
    // that relative paths and `..` traversals cannot bypass glob patterns.
    if matches!(name, "read_file" | "write_file" | "edit_file") {
        if let Some(raw) = args["path"].as_str() {
            let resolved = resolve_path(Some(raw), session_cwd);
            if is_path_denied(&resolved, config) {
                return ToolResult::Text(format!(
                    "{{\"error\": \"access to path '{}' is denied by policy\"}}",
                    resolved
                ));
            }
        }
    }
    if name == "list_dir" {
        let resolved = resolve_path(args["path"].as_str(), session_cwd);
        if is_path_denied(&resolved, config) {
            return ToolResult::Text(format!(
                "{{\"error\": \"access to path '{}' is denied by policy\"}}",
                resolved
            ));
        }
    }
    if matches!(name, "glob_files" | "grep_files") {
        let resolved = resolve_path(args["dir"].as_str(), session_cwd);
        if is_path_denied(&resolved, config) {
            return ToolResult::Text(format!(
                "{{\"error\": \"access to path '{}' is denied by policy\"}}",
                resolved
            ));
        }
    }

    // Command-based permission for shell tools
    if matches!(name, "bash" | "run_command") {
        let cmd = args["command"].as_str().unwrap_or("");
        if is_command_denied(cmd, config) {
            return ToolResult::Text(format!("{{\"error\": \"command is denied by policy\"}}"));
        }
    }

    dispatch_inner(name, args, config, session_cwd, additional_dirs).await
}

async fn dispatch_inner(
    name: &str,
    args: &serde_json::Value,
    config: &Config,
    session_cwd: Option<&Path>,
    additional_dirs: &[PathBuf],
) -> ToolResult {
    // Computer tools are not in BuiltinTool — handle them first
    match name {
        "screenshot" => {
            let region = args.get("region");
            return computer::screenshot(region).await;
        }
        "click" => {
            let x = args["x"].as_i64().unwrap_or(0) as i32;
            let y = args["y"].as_i64().unwrap_or(0) as i32;
            return ToolResult::Text(computer::click(x, y).await);
        }
        "type_text" => {
            let text = args["text"].as_str().unwrap_or("");
            return ToolResult::Text(computer::type_text(text).await);
        }
        "key_press" => {
            let key = args["key"].as_str().unwrap_or("");
            let modifiers: Vec<String> = args["modifiers"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            return ToolResult::Text(computer::key_press(key, &modifiers).await);
        }
        _ => {}
    }

    match BuiltinTool::from_str(name) {
        Some(BuiltinTool::ReadFile) => {
            let raw = args["path"].as_str().filter(|s| !s.is_empty());
            match raw {
                Some(raw_path) => {
                    let offset = args["offset"].as_u64().map(|n| n as usize);
                    let limit = args["limit"].as_u64().map(|n| n as usize);
                    let path = resolve_path(raw, session_cwd);
                    let result = files::read_file(&path, offset, limit).await;
                    // Fallback: if not found and path was relative, try additional dirs
                    if result.contains("No such file or directory")
                        && !Path::new(raw_path).is_absolute()
                    {
                        if let Some(fallback) =
                            resolve_path_with_fallback(raw_path, additional_dirs, config)
                        {
                            return ToolResult::Text(
                                files::read_file(&fallback, offset, limit).await,
                            );
                        }
                        return ToolResult::Text(format_not_found_hint(&result, additional_dirs));
                    }
                    ToolResult::Text(result)
                }
                None => ToolResult::Text("error: path is required".to_string()),
            }
        }
        Some(BuiltinTool::WriteFile) => {
            let raw = args["path"].as_str().filter(|s| !s.is_empty());
            match raw {
                Some(_) => {
                    let path = resolve_path(raw, session_cwd);
                    ToolResult::Text(
                        files::write_file(&path, args["content"].as_str().unwrap_or("")).await,
                    )
                }
                None => ToolResult::Text("error: path is required".to_string()),
            }
        }
        Some(BuiltinTool::ListDir) => {
            let raw = args["path"].as_str();
            let path = resolve_path(raw, session_cwd);
            let result = files::list_dir(&path).await;
            // Fallback: if not found and path was relative, try additional dirs
            if result.contains("No such file or directory") {
                if let Some(raw_path) = raw.filter(|p| !p.is_empty() && !Path::new(p).is_absolute())
                {
                    if let Some(fallback) =
                        resolve_path_with_fallback(raw_path, additional_dirs, config)
                    {
                        return ToolResult::Text(files::list_dir(&fallback).await);
                    }
                    return ToolResult::Text(format_not_found_hint(&result, additional_dirs));
                }
            }
            ToolResult::Text(result)
        }
        Some(BuiltinTool::RunCommand) => {
            let cmd = args["command"].as_str().unwrap_or("");
            let raw_args = crate::common::parse_str_array(&args["args"]);
            let timeout = args["timeout_secs"].as_u64().unwrap_or(30).min(120);
            let cwd = resolve_shell_cwd(args["cwd"].as_str(), session_cwd);
            ToolResult::Text(shell::run_command(cmd, &raw_args, timeout, cwd.as_deref()).await)
        }
        Some(BuiltinTool::Bash) => {
            let cmd = args["command"].as_str().unwrap_or("");
            let timeout = args["timeout_secs"].as_u64().unwrap_or(30).min(120);
            let cwd = resolve_shell_cwd(args["cwd"].as_str(), session_cwd);
            ToolResult::Text(shell::bash(cmd, timeout, cwd.as_deref()).await)
        }
        Some(BuiltinTool::EditFile) => {
            let raw = args["path"].as_str().filter(|s| !s.is_empty());
            match raw {
                Some(raw_path) => {
                    let path = resolve_path(raw, session_cwd);
                    let old_string = args["old_string"].as_str().unwrap_or("");
                    let new_string = args["new_string"].as_str().unwrap_or("");
                    let result = files::edit_file(&path, old_string, new_string).await;
                    // Fallback: if not found and path was relative, try additional dirs
                    if result.contains("No such file or directory")
                        && !Path::new(raw_path).is_absolute()
                    {
                        if let Some(fallback) =
                            resolve_path_with_fallback(raw_path, additional_dirs, config)
                        {
                            return ToolResult::Text(
                                files::edit_file(&fallback, old_string, new_string).await,
                            );
                        }
                        return ToolResult::Text(format_not_found_hint(&result, additional_dirs));
                    }
                    ToolResult::Text(result)
                }
                None => ToolResult::Text("error: path is required".to_string()),
            }
        }
        Some(BuiltinTool::GlobFiles) => {
            let pattern = args["pattern"].as_str().unwrap_or("");
            let dir = resolve_path(args["dir"].as_str(), session_cwd);
            let max_results = args["max_results"].as_u64().unwrap_or(100) as usize;
            let sort_by = args["sort_by"].as_str();
            let result = search::glob_files(pattern, &dir, max_results, sort_by);
            // Hint: if dir was not explicitly given (defaulted to session_cwd) and results are
            // empty, suggest searching additional dirs.
            if args["dir"].as_str().map_or(true, |s| s.is_empty()) {
                ToolResult::Text(format_empty_search_hint(&result, additional_dirs))
            } else {
                ToolResult::Text(result)
            }
        }
        Some(BuiltinTool::GrepFiles) => {
            let pattern = args["pattern"].as_str().unwrap_or("");
            let dir = resolve_path(args["dir"].as_str(), session_cwd);
            let file_glob = args["file_glob"].as_str();
            let max_results = args["max_results"].as_u64().unwrap_or(50) as usize;
            let output_mode = args["output_mode"].as_str();
            let context_lines = args["context"].as_u64().map(|n| n as usize);
            let result = search::grep_files(
                pattern,
                &dir,
                file_glob,
                max_results,
                output_mode,
                context_lines,
            );
            // Hint: if dir was not explicitly given and results are empty, suggest additional dirs.
            if args["dir"].as_str().map_or(true, |s| s.is_empty()) {
                ToolResult::Text(format_empty_search_hint(&result, additional_dirs))
            } else {
                ToolResult::Text(result)
            }
        }
        Some(BuiltinTool::WebFetch) => {
            let url = args["url"].as_str().unwrap_or("");
            ToolResult::Text(web::web_fetch(url).await)
        }
        Some(
            BuiltinTool::SaveMemory
            | BuiltinTool::SpawnAgent
            | BuiltinTool::Recall
            | BuiltinTool::RecordDecision
            | BuiltinTool::Skill
            | BuiltinTool::CheckBackgroundTasks
            | BuiltinTool::AddDir
            | BuiltinTool::AskHuman
            | BuiltinTool::ReportBlocker
            | BuiltinTool::ReportFinding,
        ) => {
            // Intercepted before dispatch() in the agent loop; reaching here means
            // the caller invoked dispatch() directly without agent context.
            ToolResult::Text(format!(
                "{{\"error\": \"tool '{name}' requires agent loop context\"}}"
            ))
        }
        None => ToolResult::Text(format!("unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CompressionConfig, Config, ExtensionConfig, GatewayConfig, ModelConfig, SessionConfig,
        SandboxMode,
    };
    use tempfile::TempDir;

    fn default_config() -> Config {
        Config {
            gateway: GatewayConfig {
                url: "http://localhost:9100".to_string(),
                api_key: String::new(),
                max_retries: 0,
            },
            model: ModelConfig {
                model: "test".to_string(),
                max_turns: 10,
                max_cost_usd: None,
                max_tool_output: 32_768,
                system_prompt: None,
                exploration_nudge_turns: 0,
            },
            session: SessionConfig {
                sessions_dir: PathBuf::from("/tmp"),
                show_cost: false,
                persona: None,
                allowed_tools: None,
                denied_tools: Vec::new(),
                denied_paths: Vec::new(),
                denied_commands: Vec::new(),
                sandbox_mode: SandboxMode::None,
                max_session_messages: 100,
                max_sessions: 10,
                compile_check_command: None,
                compile_check_timeout: 30,
            },
            compression: CompressionConfig {
                model: None,
                threshold: 0.8,
                preserve_recent: 5,
            },
            extensions: ExtensionConfig {
                mcp_config_path: PathBuf::from("/tmp"),
                hooks_config_path: PathBuf::from("/tmp"),
                permission_mode: None,
                plan_file: None,
            },
        }
    }

    #[test]
    fn fallback_finds_file_in_additional_dir() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("foo.txt");
        std::fs::write(&file_path, "content").unwrap();

        let additional_dirs = vec![dir.path().to_path_buf()];
        let config = default_config();

        let result = resolve_path_with_fallback("foo.txt", &additional_dirs, &config);
        assert_eq!(result, Some(file_path.to_string_lossy().into_owned()));
    }

    #[test]
    fn fallback_returns_none_when_absent() {
        let dir = TempDir::new().unwrap();
        let additional_dirs = vec![dir.path().to_path_buf()];
        let config = default_config();

        let result = resolve_path_with_fallback("nonexistent.txt", &additional_dirs, &config);
        assert!(result.is_none());
    }

    #[test]
    fn fallback_skips_denied_paths() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("secret.txt");
        std::fs::write(&file_path, "secret").unwrap();

        let additional_dirs = vec![dir.path().to_path_buf()];
        let mut config = default_config();
        // Deny everything in the temp dir
        config.session.denied_paths = vec![format!("{}/**", dir.path().display())];

        let result = resolve_path_with_fallback("secret.txt", &additional_dirs, &config);
        assert!(result.is_none());
    }

    #[test]
    fn not_found_hint_includes_dirs() {
        let dirs = vec![PathBuf::from("/workspace/b"), PathBuf::from("/workspace/c")];
        let hint = format_not_found_hint("error reading foo: No such file or directory (os error 2)", &dirs);
        assert!(hint.contains("Hint:"));
        assert!(hint.contains("/workspace/b"));
        assert!(hint.contains("/workspace/c"));
    }

    #[test]
    fn not_found_hint_no_dirs_unchanged() {
        let err = "error reading foo: No such file or directory (os error 2)";
        let hint = format_not_found_hint(err, &[]);
        assert_eq!(hint, err);
    }

    #[test]
    fn empty_search_hint_appended_for_empty_results() {
        let dirs = vec![PathBuf::from("/workspace/b")];
        let hint = format_empty_search_hint("[]", &dirs);
        assert!(hint.contains("Hint:"));
        assert!(hint.contains("/workspace/b"));
    }

    #[test]
    fn empty_search_hint_not_appended_for_non_empty() {
        let dirs = vec![PathBuf::from("/workspace/b")];
        let result = r#"["file.rs"]"#;
        let hint = format_empty_search_hint(result, &dirs);
        assert_eq!(hint, result);
    }

    #[tokio::test]
    async fn dispatch_read_file_resolves_via_fallback() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("bar.txt");
        std::fs::write(&file_path, "hello from fallback").unwrap();

        let additional_dirs = vec![dir.path().to_path_buf()];
        let config = default_config();
        let args = serde_json::json!({"path": "bar.txt"});

        // Session cwd is some other directory where bar.txt does not exist
        let other_dir = TempDir::new().unwrap();
        let result = dispatch("read_file", &args, &config, Some(other_dir.path()), None, &additional_dirs).await;
        let text = result.into_text();
        assert!(text.contains("hello from fallback"), "got: {text}");
    }

    #[tokio::test]
    async fn dispatch_read_file_hint_when_not_found_anywhere() {
        let dir = TempDir::new().unwrap();
        let additional_dirs = vec![dir.path().to_path_buf()];
        let config = default_config();
        let args = serde_json::json!({"path": "missing.txt"});

        let other_dir = TempDir::new().unwrap();
        let result = dispatch("read_file", &args, &config, Some(other_dir.path()), None, &additional_dirs).await;
        let text = result.into_text();
        assert!(text.contains("Hint:"), "expected hint, got: {text}");
    }

    #[tokio::test]
    async fn dispatch_glob_empty_result_hint() {
        let dir = TempDir::new().unwrap();
        let additional_dirs = vec![dir.path().to_path_buf()];
        let config = default_config();
        let args = serde_json::json!({"pattern": "*.nonexistent"});

        let other_dir = TempDir::new().unwrap();
        let result = dispatch("glob_files", &args, &config, Some(other_dir.path()), None, &additional_dirs).await;
        let text = result.into_text();
        assert!(text.contains("Hint:"), "expected hint, got: {text}");
    }
}

fn make_tool(name: &str, description: &str, parameters: serde_json::Value) -> Tool {
    Tool {
        r#type: "function".to_string(),
        function: ToolFunction {
            name: name.to_string(),
            description: Some(description.to_string()),
            parameters: Some(parameters),
        },
    }
}
