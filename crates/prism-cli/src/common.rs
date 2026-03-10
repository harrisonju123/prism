use prism_types::{Message, MessageRole};
use serde_json::json;
use std::collections::HashMap;

use crate::tools::BuiltinTool;

pub const SYSTEM_PROMPT: &str = "\
You are PrisM Code Agent, an autonomous coding assistant. \
You have access to tools to read, edit, and run code. \
Complete the task fully — don't stop to ask for confirmation unless truly stuck.

## Available tools

- **read_file** path [offset] [limit]: Read a file. Use offset/limit (1-based line numbers) \
  to read a section of a large file instead of the whole thing.
- **write_file** path content: Write (or overwrite) a file. Creates parent dirs automatically.
- **edit_file** path old_string new_string: Replace an exact string in a file. \
  Fails if old_string is not found or appears more than once — add more surrounding context.
- **list_dir** path: List directory contents.
- **bash** command [timeout_secs] [cwd]: Run a shell command. Returns {exit_code, stdout, stderr}. \
  Prefer this for builds, tests, git operations, and multi-step shell pipelines.
- **run_command** command args [timeout_secs] [cwd]: Run a command with separate args array. \
  Same output as bash. Use bash for most cases.
- **glob_files** pattern [dir]: Find files matching a glob (e.g. '**/*.rs').
- **grep_files** pattern [dir] [file_glob]: Search file contents by regex.
- **web_fetch** url: Fetch a URL and return its text (HTML stripped). \
  Use for documentation, crate pages, or any web resource.

## Guidelines

- Read before editing — understand the file first.
- Use bash for compiling, testing, and running programs.
- Use grep_files + glob_files to navigate unfamiliar codebases before reading individual files.
- Use read_file with offset+limit for large files — avoid reading thousands of lines you don't need.
- When done, provide a concise summary of what was changed and why.
";

// --- System prompt assembly ---

/// Build a full system prompt from components. All sections are optional
/// except `base` (which falls back to SYSTEM_PROMPT if None).
pub fn build_system_prompt(
    base: Option<&str>,
    memory: &str,
    cwd_section: &str,
    mcp_section: &str,
) -> String {
    let base = base.unwrap_or(SYSTEM_PROMPT);
    if memory.is_empty() {
        format!("{base}{cwd_section}{mcp_section}")
    } else {
        format!("## Persistent Memory\n{memory}\n\n---\n\n{base}{cwd_section}{mcp_section}")
    }
}

/// Build a system Message from the given prompt string.
pub fn system_message(prompt: String) -> Message {
    Message {
        role: MessageRole::System,
        content: Some(json!(prompt)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    }
}

/// Build a user Message from the given content string.
pub fn user_message(content: String) -> Message {
    Message {
        role: MessageRole::User,
        content: Some(json!(content)),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Default::default(),
    }
}

// --- SSE tool call accumulation ---

pub struct ToolCallBuilder {
    pub id: String,
    pub tc_type: String,
    pub name: String,
    pub arguments_buf: String,
}

impl ToolCallBuilder {
    fn new() -> Self {
        Self {
            id: String::new(),
            tc_type: "function".to_string(),
            name: String::new(),
            arguments_buf: String::new(),
        }
    }
}

/// Accumulate a single SSE tool_calls chunk delta into the builder map.
pub fn accumulate_tool_call_deltas(
    tc_arr: &[serde_json::Value],
    builders: &mut HashMap<usize, ToolCallBuilder>,
) {
    for tc in tc_arr {
        let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
        let builder = builders.entry(idx).or_insert_with(ToolCallBuilder::new);

        if let Some(id) = tc.get("id").and_then(|v| v.as_str())
            && !id.is_empty()
        {
            builder.id = id.to_string();
        }
        if let Some(t) = tc.get("type").and_then(|v| v.as_str())
            && !t.is_empty()
        {
            builder.tc_type = t.to_string();
        }
        if let Some(fname) = tc
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(|v| v.as_str())
            && !fname.is_empty()
        {
            builder.name = fname.to_string();
        }
        if let Some(args_frag) = tc
            .get("function")
            .and_then(|f| f.get("arguments"))
            .and_then(|v| v.as_str())
        {
            builder.arguments_buf.push_str(args_frag);
        }
    }
}

/// Reconstruct a sorted Vec of tool call JSON values from the builder map.
pub fn reconstruct_tool_calls(
    builders: &HashMap<usize, ToolCallBuilder>,
) -> Option<Vec<serde_json::Value>> {
    if builders.is_empty() {
        return None;
    }
    let mut indices: Vec<usize> = builders.keys().cloned().collect();
    indices.sort_unstable();
    Some(
        indices
            .iter()
            .map(|i| {
                let b = &builders[i];
                json!({
                    "id": b.id,
                    "type": b.tc_type,
                    "function": {
                        "name": b.name,
                        "arguments": b.arguments_buf
                    }
                })
            })
            .collect(),
    )
}

// --- String truncation helpers ---

/// Truncate a string to `limit` bytes at a char boundary, appending "…" if truncated.
pub fn truncate_with_ellipsis(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        s.to_string()
    } else {
        format!("{}…", &s[..s.floor_char_boundary(limit)])
    }
}

// --- Tool output truncation ---

/// Truncate tool output to fit within `limit` bytes, preserving head and tail
/// with an omission notice in the middle. For bash/run_command, truncates
/// stdout and stderr fields individually to keep JSON valid.
pub fn truncate_tool_output(tool_name: &str, output: &str, limit: usize) -> String {
    if limit == 0 || output.len() <= limit {
        return output.to_string();
    }

    if matches!(
        BuiltinTool::from_str(tool_name),
        Some(BuiltinTool::RunCommand | BuiltinTool::Bash)
    ) && let Ok(mut val) = serde_json::from_str::<serde_json::Value>(output)
    {
        let field_limit = limit / 2;
        for field in ["stdout", "stderr"] {
            if let Some(s) = val.get(field).and_then(|v| v.as_str())
                && s.len() > field_limit
            {
                let head_end = s.floor_char_boundary(field_limit * 2 / 3);
                let tail_len = (field_limit / 3).min(s.len());
                let tail_start = snap_to_char_boundary_right(s, s.len() - tail_len);
                let omitted = tail_start.saturating_sub(head_end);
                let truncated = format!(
                    "{}\n[... {omitted} chars omitted ...]\n{}",
                    &s[..head_end],
                    &s[tail_start..]
                );
                val[field] = serde_json::Value::String(truncated);
            }
        }
        if let Ok(s) = serde_json::to_string(&val) {
            return s;
        }
    }

    let head_end = output.floor_char_boundary(limit * 2 / 3);
    let tail_len = (limit / 3).min(output.len());
    let tail_start = snap_to_char_boundary_right(output, output.len() - tail_len);
    let omitted = tail_start.saturating_sub(head_end);
    format!(
        "{}\n[... {omitted} chars omitted — use a line range or narrower query to see more ...]\n{}",
        &output[..head_end],
        &output[tail_start..]
    )
}

/// Find the nearest char boundary at or after `pos`.
fn snap_to_char_boundary_right(s: &str, pos: usize) -> usize {
    let pos = pos.min(s.len());
    let mut p = pos;
    while p < s.len() && !s.is_char_boundary(p) {
        p += 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- system prompt tests ---

    #[test]
    fn build_system_prompt_no_memory() {
        let result = build_system_prompt(Some("base"), "", "", "");
        assert_eq!(result, "base");
    }

    #[test]
    fn build_system_prompt_with_memory() {
        let result = build_system_prompt(Some("base"), "mem", "", "");
        assert!(result.starts_with("## Persistent Memory\nmem"));
        assert!(result.contains("base"));
    }

    #[test]
    fn build_system_prompt_with_all_sections() {
        let result = build_system_prompt(Some("base"), "", " cwd", " mcp");
        assert_eq!(result, "base cwd mcp");
    }

    #[test]
    fn build_system_prompt_default_base() {
        let result = build_system_prompt(None, "", "", "");
        assert!(result.contains("PrisM Code Agent"));
    }

    // --- tool call accumulation tests ---

    #[test]
    fn accumulate_single_tool_call() {
        let mut builders = HashMap::new();
        let deltas = vec![json!({
            "index": 0,
            "id": "tc_1",
            "type": "function",
            "function": { "name": "read_file", "arguments": "{\"path\":" }
        })];
        accumulate_tool_call_deltas(&deltas, &mut builders);

        let deltas2 = vec![json!({
            "index": 0,
            "function": { "arguments": "\"foo.rs\"}" }
        })];
        accumulate_tool_call_deltas(&deltas2, &mut builders);

        assert_eq!(builders.len(), 1);
        let b = &builders[&0];
        assert_eq!(b.id, "tc_1");
        assert_eq!(b.name, "read_file");
        assert_eq!(b.arguments_buf, "{\"path\":\"foo.rs\"}");
    }

    #[test]
    fn reconstruct_empty_builders() {
        let builders = HashMap::new();
        assert!(reconstruct_tool_calls(&builders).is_none());
    }

    #[test]
    fn reconstruct_preserves_index_order() {
        let mut builders = HashMap::new();
        builders.insert(
            2,
            ToolCallBuilder {
                id: "tc_2".into(),
                tc_type: "function".into(),
                name: "b".into(),
                arguments_buf: "{}".into(),
            },
        );
        builders.insert(
            0,
            ToolCallBuilder {
                id: "tc_0".into(),
                tc_type: "function".into(),
                name: "a".into(),
                arguments_buf: "{}".into(),
            },
        );

        let result = reconstruct_tool_calls(&builders).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["function"]["name"], "a");
        assert_eq!(result[1]["function"]["name"], "b");
    }

    // --- truncation tests ---

    #[test]
    fn short_output_unchanged() {
        let s = "hello world";
        assert_eq!(truncate_tool_output("read_file", s, 100), s);
    }

    #[test]
    fn long_output_truncated() {
        let s = "a".repeat(1000);
        let result = truncate_tool_output("read_file", &s, 100);
        assert!(result.len() < 1000);
        assert!(result.contains("chars omitted"));
        assert!(result.starts_with('a'));
        assert!(result.ends_with('a'));
    }

    #[test]
    fn run_command_json_stays_valid() {
        let stdout = "x".repeat(40_000);
        let stderr = "e".repeat(40_000);
        let input = json!({
            "exit_code": 0,
            "stdout": stdout,
            "stderr": stderr,
        })
        .to_string();

        let result = truncate_tool_output("run_command", &input, 32_768);
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("must be valid JSON");
        assert_eq!(parsed["exit_code"], 0);
        assert!(parsed["stdout"].as_str().unwrap().contains("chars omitted"));
        assert!(parsed["stderr"].as_str().unwrap().contains("chars omitted"));
    }

    #[test]
    fn non_ascii_no_panic() {
        let s = "🦀".repeat(10_000);
        let result = truncate_tool_output("read_file", &s, 100);
        assert!(result.contains("chars omitted"));
    }

    #[test]
    fn zero_limit_is_noop() {
        let output = "some output";
        assert_eq!(truncate_tool_output("bash", output, 0), output);
    }

    #[test]
    fn multibyte_utf8_in_bash_json() {
        let output = "🔥".repeat(100);
        let result = truncate_tool_output("read_file", &output, 100);
        assert!(result.len() < 400);
        assert!(result.contains("[..."));
    }

    #[test]
    fn bash_json_truncation() {
        let long_stdout = "x".repeat(500);
        let output = json!({"exit_code": 0, "stdout": long_stdout, "stderr": ""}).to_string();
        let result = truncate_tool_output("bash", &output, 200);
        assert!(result.len() < output.len());
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["stdout"].as_str().unwrap().contains("[..."));
    }

    #[test]
    fn snap_to_char_boundary_right_works() {
        let s = "hello 🌍 world";
        let boundary = snap_to_char_boundary_right(s, 7);
        assert!(s.is_char_boundary(boundary));
        assert!(boundary >= 7);

        assert_eq!(snap_to_char_boundary_right(s, 0), 0);
        assert_eq!(snap_to_char_boundary_right(s, s.len()), s.len());
        assert_eq!(snap_to_char_boundary_right(s, s.len() + 10), s.len());
    }
}
