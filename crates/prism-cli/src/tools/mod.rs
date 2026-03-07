mod files;
mod search;
mod shell;

use prism_types::{Tool, ToolFunction};
use serde_json::json;

pub fn tool_definitions() -> Vec<Tool> {
    vec![
        make_tool(
            "read_file",
            "Read file contents",
            json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
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
                "command": { "type": "string" },
                "args": { "type": "array", "items": { "type": "string" } },
                "timeout_secs": { "type": "integer" }
            }, "required": ["command"] }),
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
    ]
}

pub async fn dispatch(name: &str, args: &serde_json::Value) -> String {
    match name {
        "read_file" => files::read_file(args["path"].as_str().unwrap_or("")).await,
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
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let timeout = args["timeout_secs"].as_u64().unwrap_or(30).min(120);
            shell::run_command(cmd, &raw_args, timeout).await
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
