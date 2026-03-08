mod files;
mod search;
mod shell;
mod web;

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
    ]
}

pub async fn dispatch(name: &str, args: &serde_json::Value) -> String {
    match name {
        "read_file" => {
            let offset = args["offset"].as_u64().map(|n| n as usize);
            let limit = args["limit"].as_u64().map(|n| n as usize);
            files::read_file(args["path"].as_str().unwrap_or(""), offset, limit).await
        }
        "write_file" => {
            files::write_file(
                args["path"].as_str().unwrap_or(""),
                args["content"].as_str().unwrap_or(""),
            )
            .await
        }
        "list_dir" => files::list_dir(args["path"].as_str().unwrap_or(".")).await,
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
            let cwd = args["cwd"].as_str();
            shell::run_command(cmd, &raw_args, timeout, cwd).await
        }
        "bash" => {
            let cmd = args["command"].as_str().unwrap_or("");
            let timeout = args["timeout_secs"].as_u64().unwrap_or(30).min(120);
            let cwd = args["cwd"].as_str();
            shell::bash(cmd, timeout, cwd).await
        }
        "edit_file" => {
            let path = args["path"].as_str().unwrap_or("");
            let old_string = args["old_string"].as_str().unwrap_or("");
            let new_string = args["new_string"].as_str().unwrap_or("");
            files::edit_file(path, old_string, new_string).await
        }
        "glob_files" => {
            let pattern = args["pattern"].as_str().unwrap_or("");
            let dir = args["dir"].as_str().unwrap_or(".");
            let max_results = args["max_results"].as_u64().unwrap_or(100) as usize;
            search::glob_files(pattern, dir, max_results)
        }
        "grep_files" => {
            let pattern = args["pattern"].as_str().unwrap_or("");
            let dir = args["dir"].as_str().unwrap_or(".");
            let file_glob = args["file_glob"].as_str();
            let max_results = args["max_results"].as_u64().unwrap_or(50) as usize;
            search::grep_files(pattern, dir, file_glob, max_results)
        }
        "web_fetch" => {
            let url = args["url"].as_str().unwrap_or("");
            web::web_fetch(url).await
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
