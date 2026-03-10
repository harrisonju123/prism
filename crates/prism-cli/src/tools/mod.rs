mod files;
pub mod computer;
mod search;
mod shell;
mod web;

use std::path::Path;

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
    Skill,
    CheckBackgroundTasks,
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
            "skill" => Some(Self::Skill),
            "check_background_tasks" => Some(Self::CheckBackgroundTasks),
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
            Self::Skill => "skill",
            Self::CheckBackgroundTasks => "check_background_tasks",
        }
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
            "Spawn a sub-agent to execute a task. The sub-agent runs independently and returns a JSON result with status, summary, and cost. Set run_in_background=true to fire-and-forget — you'll be notified on completion.",
            json!({ "type": "object", "properties": {
                "task":              { "type": "string", "description": "Natural language task for the sub-agent" },
                "model":             { "type": "string", "description": "Model to use (optional, defaults to parent model)" },
                "cost_cap":          { "type": "number", "description": "Max cost in USD (optional)" },
                "timeout_secs":      { "type": "integer", "description": "Timeout in seconds (default 300)" },
                "run_in_background": { "type": "boolean", "description": "If true, run in background and return immediately. You'll be notified when it completes." }
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
    if config.sandbox_mode == SandboxMode::ReadOnly && WRITE_TOOLS.contains(&name) {
        return false;
    }
    // Restricted mode: enforce allow/deny lists
    if config.sandbox_mode == SandboxMode::Restricted {
        if let Some(allowed) = &config.allowed_tools {
            if !allowed.iter().any(|a| a == name) {
                return false;
            }
        }
    }
    // Deny list always applies
    if config.denied_tools.iter().any(|d| d == name) {
        return false;
    }
    true
}

/// Check whether a file path is denied by the config.
pub fn is_path_denied(path: &str, config: &Config) -> bool {
    use globset::Glob;
    let path_norm = shellexpand::tilde(path).into_owned();
    for pattern in &config.denied_paths {
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
pub fn is_command_denied(cmd: &str, config: &Config) -> bool {
    let cmd_trimmed = cmd.trim();
    for denied in &config.denied_commands {
        if cmd_trimmed.starts_with(denied.trim()) {
            return true;
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

pub async fn dispatch(
    name: &str,
    args: &serde_json::Value,
    config: &Config,
    session_cwd: Option<&Path>,
    mcp: Option<&McpRegistry>,
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

    // Path-based permission for file tools
    if matches!(name, "read_file" | "write_file" | "edit_file") {
        if let Some(path) = args["path"].as_str() {
            if is_path_denied(path, config) {
                return ToolResult::Text(format!(
                    "{{\"error\": \"access to path '{}' is denied by policy\"}}", path
                ));
            }
        }
    }

    // Command-based permission for shell tools
    if matches!(name, "bash" | "run_command") {
        let cmd = args["command"].as_str().unwrap_or("");
        if is_command_denied(cmd, config) {
            return ToolResult::Text(format!(
                "{{\"error\": \"command is denied by policy\"}}"
            ));
        }
    }

    dispatch_inner(name, args, session_cwd).await
}

async fn dispatch_inner(name: &str, args: &serde_json::Value, session_cwd: Option<&Path>) -> ToolResult {
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
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            return ToolResult::Text(computer::key_press(key, &modifiers).await);
        }
        _ => {}
    }

    match BuiltinTool::from_str(name) {
        Some(BuiltinTool::ReadFile) => {
            let raw = args["path"].as_str().filter(|s| !s.is_empty());
            match raw {
                Some(_) => {
                    let offset = args["offset"].as_u64().map(|n| n as usize);
                    let limit = args["limit"].as_u64().map(|n| n as usize);
                    let path = resolve_path(raw, session_cwd);
                    ToolResult::Text(files::read_file(&path, offset, limit).await)
                }
                None => ToolResult::Text("error: path is required".to_string()),
            }
        }
        Some(BuiltinTool::WriteFile) => {
            let raw = args["path"].as_str().filter(|s| !s.is_empty());
            match raw {
                Some(_) => {
                    let path = resolve_path(raw, session_cwd);
                    ToolResult::Text(files::write_file(&path, args["content"].as_str().unwrap_or("")).await)
                }
                None => ToolResult::Text("error: path is required".to_string()),
            }
        }
        Some(BuiltinTool::ListDir) => {
            let path = resolve_path(args["path"].as_str(), session_cwd);
            ToolResult::Text(files::list_dir(&path).await)
        }
        Some(BuiltinTool::RunCommand) => {
            let cmd = args["command"].as_str().unwrap_or("");
            let raw_args: Vec<String> = args["args"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
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
                Some(_) => {
                    let path = resolve_path(raw, session_cwd);
                    let old_string = args["old_string"].as_str().unwrap_or("");
                    let new_string = args["new_string"].as_str().unwrap_or("");
                    ToolResult::Text(files::edit_file(&path, old_string, new_string).await)
                }
                None => ToolResult::Text("error: path is required".to_string()),
            }
        }
        Some(BuiltinTool::GlobFiles) => {
            let pattern = args["pattern"].as_str().unwrap_or("");
            let dir = resolve_path(args["dir"].as_str(), session_cwd);
            let max_results = args["max_results"].as_u64().unwrap_or(100) as usize;
            let sort_by = args["sort_by"].as_str();
            ToolResult::Text(search::glob_files(pattern, &dir, max_results, sort_by))
        }
        Some(BuiltinTool::GrepFiles) => {
            let pattern = args["pattern"].as_str().unwrap_or("");
            let dir = resolve_path(args["dir"].as_str(), session_cwd);
            let file_glob = args["file_glob"].as_str();
            let max_results = args["max_results"].as_u64().unwrap_or(50) as usize;
            let output_mode = args["output_mode"].as_str();
            let context_lines = args["context"].as_u64().map(|n| n as usize);
            ToolResult::Text(search::grep_files(
                pattern,
                &dir,
                file_glob,
                max_results,
                output_mode,
                context_lines,
            ))
        }
        Some(BuiltinTool::WebFetch) => {
            let url = args["url"].as_str().unwrap_or("");
            ToolResult::Text(web::web_fetch(url).await)
        }
        Some(BuiltinTool::SaveMemory | BuiltinTool::SpawnAgent | BuiltinTool::Recall | BuiltinTool::Skill | BuiltinTool::CheckBackgroundTasks) => {
            // Intercepted before dispatch() in the agent loop; reaching here means
            // the caller invoked dispatch() directly without agent context.
            ToolResult::Text(format!("{{\"error\": \"tool '{name}' requires agent loop context\"}}"))
        }
        None => ToolResult::Text(format!("unknown tool: {name}")),
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
