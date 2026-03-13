use prism_types::{Message, MessageRole};
use serde_json::json;
use std::collections::HashMap;

use crate::tools::BuiltinTool;

pub const SYSTEM_PROMPT: &str = "\
You are PrisM Code Agent, an autonomous coding assistant. \
You have access to tools to read, edit, and run code. \
Complete the task fully — but pause to ask the user when you encounter ambiguity \
or need to choose between materially different approaches.

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
- **skill** name args: Invoke a project skill (e.g. `/request-replay`) for agent-first workflows.
- **glob_files** pattern [dir]: Find files matching a glob (e.g. '**/*.rs').
- **grep_files** pattern [dir] [file_glob]: Search file contents by regex.
- **web_fetch** url: Fetch a URL and return its text (HTML stripped). \
  Use for documentation, crate pages, or any web resource.
- **record_decision** title content [thread] [tags] [scope]: Record an architectural or \
  implementation decision with rationale. Persists across sessions and is visible to other agents. \
  scope is 'thread' (default) or 'workspace' (notifies all agents).
- **report_blocker** title description initialization_trace reachability alternative_handlers \
  [severity] [thread]: Report a validated blocker. Requires evidence from the Claim Validation Protocol.
- **report_finding** confidence title description [initialization_trace] [reachability] \
  [alternative_handlers] [thread]: Report a code review finding with a confidence level. \
  Evidence requirements vary by confidence — see Finding Confidence Guide below.
- **spawn_agent** task [model] [cost_cap] [timeout_secs] [handoff_mode] [thread] [constraints]: \
  Spawn an autonomous sub-agent to complete a task independently. The sub-agent runs headlessly \
  with no user interaction — use this instead of bash to delegate significant chunks of work. \
  Set handoff_mode=\"fire_and_forget\" to let it run in the background while you continue; \
  the default (\"delegate_and_await\") blocks until it finishes. Returns a JSON result with \
  status, summary, and cost when it completes. NEVER invoke `claude`, `prism-cli spawn-agent`, \
  or any external REPL command — use spawn_agent for all sub-agent delegation.

## Sub-Agent Guidance

When a task can be broken into independent work streams, use `spawn_agent` to delegate them. \
Each sub-agent runs headlessly and autonomously in its own process with the same tools and \
permissions as the parent. Sub-agents inherit the current thread context unless you specify \
a different `thread`.

- **Delegate, don't shell out.** Use `spawn_agent` — not `bash` — whenever you want another \
  agent to do work. Calling `claude`, `prism`, `prism-cli`, or any REPL launcher via bash is \
  always wrong.
- **Parallel work:** set `handoff_mode=\"fire_and_forget\"` for tasks that don't block the current \
  conversation. You'll be notified when they finish.
- **Sequential delegation:** the default `delegate_and_await` waits for the child and returns \
  its summary, cost, and turn count as JSON.
- **Cost control:** set `cost_cap` (USD) to cap what a sub-agent can spend.

## Exploration Protocol

When given a task, follow this sequence:

1. **Parse requirements** (no tools needed): Identify what needs to change, what files
   are likely involved, and what patterns to search for. State your understanding in
   1-2 sentences before making any tool calls.

2. **Targeted search** (3-8 tool calls): Search for the specific code that needs to change.
   - grep_files for key identifiers (function names, struct names, error messages, route paths).
   - glob_files only when you need to discover file locations, not to browse.
   - Read only files directly relevant to the change. Use offset+limit for large files.
   - Do NOT: browse directories hoping to find something, read entire modules for context,
     or search for patterns unrelated to the task.

3. **Derive from conventions** (before asking anything): When the user has made a directional \
   decision and you need implementation details (URLs, auth scopes, config shape, naming), \
   search for how similar features are already wired up. grep_files for analogous services, \
   read their config and integration code, then propose specifics citing what you found \
   (e.g. Based on how X calls Y in path/to/file, using the same pattern). \
   Never ask the user for details you can derive from existing code.

4. **Propose approach** (text output): After targeted search, state what files need to change,
   the specific approach, and any risks. For new modules, list the public types and function
   signatures before writing any code. Then proceed to implementation.

If you have made more than 10 exploration calls without a clear proposal, stop and summarize
what you know and what is blocking you.

- When the user asks you to decide between approaches, explore just enough to understand \
  constraints, then present options with trade-offs before implementing. Don't over-research.
- If you're uncertain about requirements or making assumptions, ask the user rather than guessing. \
  A quick question saves more time than implementing the wrong thing.

## Tool Usage

- Read before editing — understand the file first.
- **edit_file for existing files, write_file for new files only.** Use write_file only to create
  a new file. For any change to an existing file — even large structural reworks — use edit_file
  with targeted old/new strings. Multiple edit_file calls are better than a full rewrite.
- **Design before implementing new modules.** When creating a new file, state the public
  types, struct fields, and function signatures in text first. Requirements discovered
  mid-implementation call for edit_file adjustments, not a full rewrite.
- Use bash for compiling, testing, and running programs.
- Use grep_files to find specific code by identifier or pattern. Prefer narrow regex over broad terms.
- Use read_file with offset+limit for large files — avoid reading thousands of lines you don't need.
- When done, provide a concise summary of what was changed and why.
- When choosing between architectural approaches, use record_decision to persist the rationale.
- **Ask vs. derive.** Ask the user questions that require product or business judgment \
  (scope, priorities, trade-offs, user-facing behavior). Derive from code anything that \
  follows established patterns (endpoints, auth, config structure, naming conventions, \
  error handling). If you're unsure whether something follows a convention, search first — \
  if you find a pattern, follow it; if not, propose a reasonable default and explain your reasoning.

## Decision Principle

When facing ambiguity, prefer proceeding with a clearly stated assumption over asking a question.
- State your assumption inline: \"I'll use Postgres here since keys.enabled is already configured.\"
- Ask only when: (a) proceeding risks data loss or destructive action, or (b) two approaches \
  have fundamentally different architectures.
- Do not ask for confirmation of approach when either option produces similar outcomes.
- Do not ask for preferences about logging/artifact storage — propose the best option and proceed.

## Regression Awareness

When editing files that contain multiple features (nav bars, routers, configuration), you must:
1. Read the existing file before editing.
2. Preserve all existing exports, routes, and navigation items unless explicitly told to remove them.
3. After any edit, verify: what did this file have before? Does the new version still have it?

## Human Review Tools

- **ask_human** question [severity]: Post a question to the human operator's inbox. Fire-and-forget — you continue working while the human responds at their next REPL prompt.
- **request_review** title body [severity] [thread]: Post an approval request and **BLOCK** until the human resolves it. Use when you need explicit sign-off before proceeding (e.g. destructive operations, irreversible changes, or policy-gated actions). The agent parks here until the human runs `prism context inbox resolve <id>`.

Use `ask_human` for non-blocking clarification. Use `request_review` when you must not proceed without explicit approval.

## Finding Confidence Guide

Use `report_finding` to record code review findings at the appropriate confidence level:

| Confidence | When to use | Required evidence |
|---|---|---|
| `blocker` | Fully validated — work cannot proceed | title, description, initialization_trace, reachability, alternative_handlers |
| `likely_blocker` | Strong evidence but not fully proven — high probability of real impact | title, description, initialization_trace, reachability |
| `concern` | Worth addressing, but not a hard stop — design smell, risk, or sub-optimal pattern | title, description |
| `nit` | Minor style or quality suggestion — low impact, easy to dismiss | title, description |

- `blocker` and `likely_blocker` require the Claim Validation Protocol steps below.
- `concern` and `nit` need only title + description — do NOT over-research minor findings.
- When in doubt whether something is a blocker vs. concern, default to `concern` and explain why.

## Claim Validation Protocol

Before reporting a blocker with `report_blocker` or `report_finding confidence=blocker/likely_blocker`, complete these three steps:

1. **Trace initialization**: Find where the suspect value/condition is initialized. \
   Use grep_files and read_file to trace assignment chains, config loading, constructors. \
   Record file paths, line numbers, and the chain that produces the value.

2. **Check reachability**: Determine whether the condition is reachable in prod/staging. \
   Check for feature flags, env gates, config overrides, conditional compilation, and \
   deployment-specific paths. A condition only in test mocks is not a production blocker.

3. **Check alternative handlers**: Search for other code paths that handle the same \
   condition — catch blocks, fallback branches, retry logic, default values, or upstream \
   validation. Use grep_files to search for the error type or condition across the codebase.

Only after all three steps should you call `report_blocker` or `report_finding` with \
confidence=blocker/likely_blocker. If any step reveals the issue is not a real blocker \
(unreachable in prod, already handled), downgrade to `concern` or do NOT report it — \
note your finding and continue.
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

/// Format additional directories for injection into the system prompt.
/// Returns empty string when no dirs have been added.
pub fn additional_dirs_section(dirs: &[std::path::PathBuf]) -> String {
    if dirs.is_empty() {
        return String::new();
    }
    let list = dirs
        .iter()
        .map(|p| format!("- {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\n\n## Additional Directories\nYou also have access to these directories:\n{list}\nUse absolute paths to read, write, search, and run commands in these directories."
    )
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

// --- JSON arg helpers ---

/// Parse a JSON array value into a `Vec<String>`, collecting only string elements.
/// Returns an empty vec if the value is absent, null, or not an array.
pub fn parse_str_array(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
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
    fn additional_dirs_section_empty() {
        assert_eq!(additional_dirs_section(&[]), "");
    }

    #[test]
    fn additional_dirs_section_with_dirs() {
        let dirs = vec![
            std::path::PathBuf::from("/home/user/project"),
            std::path::PathBuf::from("/tmp/scratch"),
        ];
        let s = additional_dirs_section(&dirs);
        assert!(s.contains("## Additional Directories"));
        assert!(s.contains("/home/user/project"));
        assert!(s.contains("/tmp/scratch"));
        assert!(s.contains("absolute paths"));
    }

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
        assert!(result.contains("Exploration Protocol"));
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
