mod files;
mod search;
mod shell;
mod web;

use std::path::Path;

use prism_types::{Tool, ToolFunction};
use serde_json::json;

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
            "Find files by glob pattern (e.g. '**/*.rs'). Returns array of matching paths.",
            json!({ "type": "object", "properties": {
                "pattern":     { "type": "string" },
                "dir":         { "type": "string", "description": "root dir to search (default '.')" },
                "max_results": { "type": "integer" }
            }, "required": ["pattern"] }),
        ),
        make_tool(
            "grep_files",
            "Search file contents by regex. Returns [{path, line, text}] matches.",
            json!({ "type": "object", "properties": {
                "pattern":     { "type": "string", "description": "regex pattern" },
                "dir":         { "type": "string", "description": "root dir (default '.')" },
                "file_glob":   { "type": "string", "description": "optional glob to filter files, e.g. '*.rs'" },
                "max_results": { "type": "integer" }
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
            "Spawn a sub-agent to execute a task. The sub-agent runs independently and returns a JSON result with status, summary, and cost.",
            json!({ "type": "object", "properties": {
                "task":         { "type": "string", "description": "Natural language task for the sub-agent" },
                "model":        { "type": "string", "description": "Model to use (optional, defaults to parent model)" },
                "cost_cap":     { "type": "number", "description": "Max cost in USD (optional)" },
                "timeout_secs": { "type": "integer", "description": "Timeout in seconds (default 300)" }
            }, "required": ["task"] }),
        ),
    ]
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

pub async fn dispatch(name: &str, args: &serde_json::Value, session_cwd: Option<&Path>) -> String {
    match name {
        "read_file" => {
            let raw = args["path"].as_str().filter(|s| !s.is_empty());
            match raw {
                Some(_) => {
                    let offset = args["offset"].as_u64().map(|n| n as usize);
                    let limit = args["limit"].as_u64().map(|n| n as usize);
                    let path = resolve_path(raw, session_cwd);
                    files::read_file(&path, offset, limit).await
                }
                None => "error: path is required".to_string(),
            }
        }
        "write_file" => {
            let raw = args["path"].as_str().filter(|s| !s.is_empty());
            match raw {
                Some(_) => {
                    let path = resolve_path(raw, session_cwd);
                    files::write_file(&path, args["content"].as_str().unwrap_or("")).await
                }
                None => "error: path is required".to_string(),
            }
        }
        "list_dir" => {
            let path = resolve_path(args["path"].as_str(), session_cwd);
            files::list_dir(&path).await
        }
        "run_command" => {
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
            shell::run_command(cmd, &raw_args, timeout, cwd.as_deref()).await
        }
        "bash" => {
            let cmd = args["command"].as_str().unwrap_or("");
            let timeout = args["timeout_secs"].as_u64().unwrap_or(30).min(120);
            let cwd = resolve_shell_cwd(args["cwd"].as_str(), session_cwd);
            shell::bash(cmd, timeout, cwd.as_deref()).await
        }
        "edit_file" => {
            let raw = args["path"].as_str().filter(|s| !s.is_empty());
            match raw {
                Some(_) => {
                    let path = resolve_path(raw, session_cwd);
                    let old_string = args["old_string"].as_str().unwrap_or("");
                    let new_string = args["new_string"].as_str().unwrap_or("");
                    files::edit_file(&path, old_string, new_string).await
                }
                None => "error: path is required".to_string(),
            }
        }
        "glob_files" => {
            let pattern = args["pattern"].as_str().unwrap_or("");
            let dir = resolve_path(args["dir"].as_str(), session_cwd);
            let max_results = args["max_results"].as_u64().unwrap_or(100) as usize;
            search::glob_files(pattern, &dir, max_results)
        }
        "grep_files" => {
            let pattern = args["pattern"].as_str().unwrap_or("");
            let dir = resolve_path(args["dir"].as_str(), session_cwd);
            let file_glob = args["file_glob"].as_str();
            let max_results = args["max_results"].as_u64().unwrap_or(50) as usize;
            search::grep_files(pattern, &dir, file_glob, max_results)
        }
        "web_fetch" => {
            let url = args["url"].as_str().unwrap_or("");
            web::web_fetch(url).await
        }
        "save_memory" | "spawn_agent" => {
            // Intercepted before dispatch() in the agent loop; reaching here means
            // the caller invoked dispatch() directly without agent context.
            format!("{{\"error\": \"tool '{name}' requires agent loop context\"}}")
        }
        other => format!("unknown tool: {other}"),
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
