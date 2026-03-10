//! ACP (Agent Communication Protocol) server for PrisM.
//!
//! Runs `prism acp` as a stdio-based JSON-RPC agent server that Zed connects to.
//!
//! ## Zed Configuration
//!
//! Add to your Zed `settings.json`:
//!
//! ```json
//! {
//!   "agent_servers": {
//!     "prism": {
//!       "command": {
//!         "path": "~/.cargo/bin/prism",
//!         "args": ["acp"],
//!         "env": {
//!           "PRISM_MODEL": "claude-sonnet-4-6"
//!         }
//!       }
//!     }
//!   }
//! }
//! ```

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use agent_client_protocol as acp;
use anyhow::Result;
use futures::StreamExt;
use prism_client::PrismClient;
use prism_types::{ChatCompletionRequest, Message};
use serde_json::json;
use uuid::Uuid;

use crate::config::Config;
use crate::memory::MemoryManager;
use crate::tools;

const SYSTEM_PROMPT: &str = "\
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

struct AcpSession {
    messages: Vec<Message>,
    cancelled: Arc<AtomicBool>,
    /// Tools the user has granted "allow for session" permission
    allowed_tools: HashSet<String>,
    /// Per-session model override (set via `session/set_model`)
    model: Option<String>,
    /// Working directory for this session (from Zed's project root)
    cwd: PathBuf,
}

// SAFETY: PrismAgent uses RefCell because the ACP Agent trait is !Send and runs
// on a single-threaded LocalSet. All RefCell borrows MUST be scoped in synchronous
// blocks and dropped before any .await point. The `cancel` method only needs the
// Arc<AtomicBool> (extracted via a short-lived immutable borrow), so it cannot
// conflict with the scoped mutable borrows in run_agent_loop.
pub struct PrismAgent {
    sessions: RefCell<HashMap<String, AcpSession>>,
    connection: RefCell<Option<Rc<acp::AgentSideConnection>>>,
    config: Config,
    memory: RefCell<MemoryManager>,
}

impl PrismAgent {
    pub fn new(config: Config) -> Self {
        let memory_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".prism/memory");
        let memory = MemoryManager::new(&memory_dir, config.memory_window_size);
        Self {
            sessions: RefCell::new(HashMap::new()),
            connection: RefCell::new(None),
            config,
            memory: RefCell::new(memory),
        }
    }

    pub fn set_connection(&self, conn: acp::AgentSideConnection) {
        *self.connection.borrow_mut() = Some(Rc::new(conn));
    }

    fn connection(&self) -> Rc<acp::AgentSideConnection> {
        self.connection
            .borrow()
            .as_ref()
            .expect("connection not set")
            .clone()
    }

    async fn send_update(&self, session_id: &str, update: acp::SessionUpdate) {
        let conn = self.connection();
        let notification =
            acp::SessionNotification::new(acp::SessionId::new(session_id), update);
        if let Err(e) = acp::Client::session_notification(&*conn, notification).await {
            tracing::warn!("failed to send session update: {e:?}");
        }
    }

    async fn send_text_chunk(&self, session_id: &str, text: &str) {
        self.send_update(
            session_id,
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                acp::ContentBlock::from(text),
            )),
        )
        .await;
    }

    fn build_system_message(&self, cwd: Option<&Path>) -> Message {
        let base_system_prompt = self
            .config
            .system_prompt
            .as_deref()
            .unwrap_or(SYSTEM_PROMPT);

        let memory_content = self.memory.borrow().load();
        let cwd_section = cwd
            .map(|p| {
                format!(
                    "\n\n## Working Directory\n\nYou are working in: {}\n\
                     All relative paths in tool calls should be relative to this directory.\n\
                     Use this as the default `dir` for glob_files and grep_files, and `cwd` for bash/run_command.",
                    p.display()
                )
            })
            .unwrap_or_default();

        let full_system = if memory_content.is_empty() {
            format!("{base_system_prompt}{cwd_section}")
        } else {
            format!("## Persistent Memory\n{memory_content}\n\n---\n\n{base_system_prompt}{cwd_section}")
        };

        Message {
            role: "system".into(),
            content: Some(json!(full_system)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }
    }

    fn is_read_only(tool_name: &str) -> bool {
        matches!(
            tool_name,
            "read_file" | "list_dir" | "glob_files" | "grep_files" | "web_fetch"
        )
    }

    /// Ask the client for permission before executing a write tool.
    /// Returns true if allowed, false if rejected/cancelled.
    async fn request_tool_permission(
        &self,
        session_id: &str,
        tool_name: &str,
        tool_call_id: &acp::ToolCallId,
        title: &str,
        args: &serde_json::Value,
    ) -> bool {
        // Check if already allowed for this session
        {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(session_id)
                && session.allowed_tools.contains(tool_name)
            {
                return true;
            }
        }

        let conn = self.connection();

        let mut locations = Vec::new();
        if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
            locations.push(acp::ToolCallLocation::new(path));
        }

        let tool_update = acp::ToolCallUpdate::new(
            tool_call_id.clone(),
            acp::ToolCallUpdateFields::new()
                .title(title)
                .locations(locations),
        );

        let options = vec![
            acp::PermissionOption::new(
                acp::PermissionOptionId::new("allow-once"),
                "Allow once",
                acp::PermissionOptionKind::AllowOnce,
            ),
            acp::PermissionOption::new(
                acp::PermissionOptionId::new("allow-session"),
                format!("Allow {tool_name} for this session"),
                acp::PermissionOptionKind::AllowAlways,
            ),
            acp::PermissionOption::new(
                acp::PermissionOptionId::new("reject-once"),
                "Reject",
                acp::PermissionOptionKind::RejectOnce,
            ),
        ];

        let req = acp::RequestPermissionRequest::new(
            acp::SessionId::new(session_id),
            tool_update,
            options,
        );

        match acp::Client::request_permission(&*conn, req).await {
            Ok(resp) => match resp.outcome {
                acp::RequestPermissionOutcome::Selected(selected) => {
                    let id = selected.option_id.0.as_ref();
                    if id == "allow-session" {
                        let mut sessions = self.sessions.borrow_mut();
                        if let Some(session) = sessions.get_mut(session_id) {
                            session.allowed_tools.insert(tool_name.to_string());
                        }
                    }
                    id == "allow-once" || id == "allow-session"
                }
                acp::RequestPermissionOutcome::Cancelled => false,
                _ => false,
            },
            Err(e) => {
                tracing::warn!("permission request failed: {e:?}");
                false
            }
        }
    }

    /// Trim old messages to stay within max_session_messages, preserving the system prompt.
    fn trim_messages(messages: &mut Vec<Message>, max: usize) {
        if max == 0 || messages.len() <= max {
            return;
        }
        // Keep system prompt (index 0) + the most recent (max - 1) messages
        let drain_end = messages.len() - (max - 1);
        messages.drain(1..drain_end);
    }

    /// Evict oldest sessions when we exceed max_sessions.
    fn evict_sessions_if_needed(&self) {
        let mut sessions = self.sessions.borrow_mut();
        let max = self.config.max_sessions;
        if max == 0 || sessions.len() <= max {
            return;
        }
        let excess = sessions.len() - max;
        let keys_to_remove: Vec<String> = sessions.keys().take(excess).cloned().collect();
        for key in keys_to_remove {
            sessions.remove(&key);
        }
    }

    async fn run_agent_loop(&self, session_id: &str) -> acp::StopReason {
        let client = PrismClient::new(&self.config.prism_url)
            .with_api_key(&self.config.prism_api_key);
        let model_fallback = self.config.prism_model.clone();

        // cwd is set once at session creation and never changes
        let session_cwd = {
            let sessions = self.sessions.borrow();
            match sessions.get(session_id) {
                Some(s) => s.cwd.clone(),
                None => return acp::StopReason::EndTurn,
            }
        };

        let mut tool_call_counter: u32 = 0;

        for _turn in 0..self.config.max_turns {
            let (cancelled, messages, session_model) = {
                let sessions = self.sessions.borrow();
                match sessions.get(session_id) {
                    Some(s) => (s.cancelled.clone(), s.messages.clone(), s.model.clone()),
                    None => return acp::StopReason::EndTurn,
                }
            };

            if cancelled.load(Ordering::Acquire) {
                return acp::StopReason::Cancelled;
            }

            let req = ChatCompletionRequest {
                model: session_model.unwrap_or_else(|| model_fallback.clone()),
                messages,
                tools: Some(tools::tool_definitions()),
                tool_choice: Some(json!("auto")),
                ..Default::default()
            };

            let stream_result = client.stream_chat_completion(&req).await;
            let mut stream = match stream_result {
                Ok(s) => s,
                Err(e) => {
                    self.send_text_chunk(session_id, &format!("\n[error] {e}")).await;
                    return acp::StopReason::EndTurn;
                }
            };

            let mut content_buf = String::new();
            let mut tc_builders: HashMap<usize, ToolCallBuilder> = HashMap::new();
            let mut finish_reason: Option<String> = None;

            while let Some(chunk_result) = stream.next().await {
                if cancelled.load(Ordering::Acquire) {
                    return acp::StopReason::Cancelled;
                }

                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        self.send_text_chunk(session_id, &format!("\n[stream error] {e}"))
                            .await;
                        return acp::StopReason::EndTurn;
                    }
                };

                if !chunk.delta.is_empty() {
                    self.send_text_chunk(session_id, &chunk.delta).await;
                    content_buf.push_str(&chunk.delta);
                }

                if let Some(tc_arr) = chunk
                    .tool_calls
                    .as_ref()
                    .and_then(|v| v.as_array())
                {
                    for tc in tc_arr {
                        let idx =
                            tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let builder =
                            tc_builders.entry(idx).or_insert_with(|| ToolCallBuilder {
                                id: String::new(),
                                name: String::new(),
                                arguments_buf: String::new(),
                            });
                        if let Some(id) = tc.get("id").and_then(|v| v.as_str())
                            && !id.is_empty()
                        {
                            builder.id = id.to_string();
                        }
                        // Name only appears once per tool call in the SSE stream
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

                if chunk.finish_reason.is_some() {
                    finish_reason = chunk.finish_reason;
                }
            }

            if cancelled.load(Ordering::Acquire) {
                return acp::StopReason::Cancelled;
            }

            // Reconstruct tool_calls
            let tool_calls_vec: Option<Vec<serde_json::Value>> = if tc_builders.is_empty() {
                None
            } else {
                let mut indices: Vec<usize> = tc_builders.keys().cloned().collect();
                indices.sort_unstable();
                Some(
                    indices
                        .iter()
                        .map(|i| {
                            let b = &tc_builders[i];
                            json!({
                                "id": b.id,
                                "type": "function",
                                "function": {
                                    "name": b.name,
                                    "arguments": b.arguments_buf
                                }
                            })
                        })
                        .collect(),
                )
            };

            // Push assistant message
            {
                let mut sessions = self.sessions.borrow_mut();
                if let Some(session) = sessions.get_mut(session_id) {
                    session.messages.push(Message {
                        role: "assistant".into(),
                        content: if content_buf.is_empty() {
                            None
                        } else {
                            Some(json!(content_buf))
                        },
                        name: None,
                        tool_calls: tool_calls_vec.clone(),
                        tool_call_id: None,
                        extra: Default::default(),
                    });
                    Self::trim_messages(
                        &mut session.messages,
                        self.config.max_session_messages,
                    );
                }
            }

            match finish_reason.as_deref() {
                Some("stop") | None => {
                    return acp::StopReason::EndTurn;
                }
                Some("tool_calls") => {
                    for tc in tool_calls_vec.unwrap_or_default() {
                        if cancelled.load(Ordering::Acquire) {
                            return acp::StopReason::Cancelled;
                        }

                        let id = tc["id"].as_str().unwrap_or("").to_string();
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let args: serde_json::Value = tc["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(json!({}));

                        tool_call_counter += 1;
                        let tool_call_id =
                            acp::ToolCallId::new(format!("tc_{tool_call_counter}"));

                        let kind = tool_kind(name);
                        let title = tool_title(name, &args);

                        // Emit ToolCall (pending)
                        self.send_update(
                            session_id,
                            acp::SessionUpdate::ToolCall(
                                acp::ToolCall::new(tool_call_id.clone(), &title)
                                    .kind(kind)
                                    .status(acp::ToolCallStatus::InProgress)
                                    .raw_input(args.clone()),
                            ),
                        )
                        .await;

                        // Request permission for write tools
                        if !Self::is_read_only(name) {
                            let allowed = self
                                .request_tool_permission(
                                    session_id,
                                    name,
                                    &tool_call_id,
                                    &title,
                                    &args,
                                )
                                .await;
                            if !allowed {
                                // Emit rejection as a failed tool call
                                self.send_update(
                                    session_id,
                                    acp::SessionUpdate::ToolCallUpdate(
                                        acp::ToolCallUpdate::new(
                                            tool_call_id,
                                            acp::ToolCallUpdateFields::new()
                                                .status(acp::ToolCallStatus::Failed)
                                                .content(vec![acp::ContentBlock::from(
                                                    "Permission denied by user".to_string(),
                                                )
                                                .into()]),
                                        ),
                                    ),
                                )
                                .await;

                                // Feed rejection back to LLM as tool result
                                {
                                    let mut sessions = self.sessions.borrow_mut();
                                    if let Some(session) = sessions.get_mut(session_id) {
                                        session.messages.push(Message {
                                            role: "tool".into(),
                                            content: Some(json!(
                                                "Permission denied by user. Do not retry this tool call. \
                                                 Inform the user what you wanted to do and ask how to proceed."
                                            )),
                                            name: None,
                                            tool_calls: None,
                                            tool_call_id: Some(id),
                                            extra: Default::default(),
                                        });
                                    }
                                }
                                continue;
                            }
                        }

                        let result = if name == "save_memory" {
                            let key =
                                args["key"].as_str().unwrap_or("note").to_string();
                            let value =
                                args["value"].as_str().unwrap_or("").to_string();
                            self.memory.borrow_mut().append(key.clone(), value);
                            json!({"saved": true, "key": key}).to_string()
                        } else {
                            tools::dispatch(name, &args, Some(&session_cwd)).await
                        };
                        let result = truncate_tool_output(
                            name,
                            &result,
                            self.config.max_tool_output,
                        );

                        // Push tool result to messages before sending update to avoid clone
                        {
                            let mut sessions = self.sessions.borrow_mut();
                            if let Some(session) = sessions.get_mut(session_id) {
                                session.messages.push(Message {
                                    role: "tool".into(),
                                    content: Some(json!(&result)),
                                    name: None,
                                    tool_calls: None,
                                    tool_call_id: Some(id),
                                    extra: Default::default(),
                                });
                            }
                        }

                        // Emit ToolCallUpdate (completed)
                        self.send_update(
                            session_id,
                            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                                tool_call_id,
                                acp::ToolCallUpdateFields::new()
                                    .status(acp::ToolCallStatus::Completed)
                                    .content(vec![acp::ContentBlock::from(result)
                                        .into()]),
                            )),
                        )
                        .await;
                    }
                }
                _ => {
                    return acp::StopReason::EndTurn;
                }
            }
        }

        // Exceeded max turns
        acp::StopReason::MaxTurnRequests
    }
}

struct ToolCallBuilder {
    id: String,
    name: String,
    arguments_buf: String,
}

fn tool_kind(name: &str) -> acp::ToolKind {
    match name {
        "read_file" | "list_dir" => acp::ToolKind::Read,
        "write_file" | "edit_file" => acp::ToolKind::Edit,
        "glob_files" | "grep_files" => acp::ToolKind::Search,
        "bash" | "run_command" => acp::ToolKind::Execute,
        "web_fetch" => acp::ToolKind::Fetch,
        "save_memory" => acp::ToolKind::Other,
        _ => acp::ToolKind::Other,
    }
}

fn tool_title(name: &str, args: &serde_json::Value) -> String {
    match name {
        "read_file" => format!(
            "Read {}",
            args["path"].as_str().unwrap_or("file")
        ),
        "write_file" => format!(
            "Write {}",
            args["path"].as_str().unwrap_or("file")
        ),
        "edit_file" => format!(
            "Edit {}",
            args["path"].as_str().unwrap_or("file")
        ),
        "list_dir" => format!(
            "List {}",
            args["path"].as_str().unwrap_or(".")
        ),
        "bash" => {
            let cmd = args["command"].as_str().unwrap_or("command");
            let preview = if cmd.len() > 60 {
                let boundary = cmd.floor_char_boundary(60);
                format!("{}…", &cmd[..boundary])
            } else {
                cmd.to_string()
            };
            format!("Run: {preview}")
        }
        "run_command" => format!(
            "Run: {}",
            args["command"].as_str().unwrap_or("command")
        ),
        "glob_files" => format!(
            "Glob {}",
            args["pattern"].as_str().unwrap_or("*")
        ),
        "grep_files" => format!(
            "Grep {}",
            args["pattern"].as_str().unwrap_or("pattern")
        ),
        "web_fetch" => format!(
            "Fetch {}",
            args["url"].as_str().unwrap_or("url")
        ),
        "save_memory" => format!(
            "Save memory: {}",
            args["key"].as_str().unwrap_or("note")
        ),
        other => other.to_string(),
    }
}

fn truncate_tool_output(tool_name: &str, output: &str, limit: usize) -> String {
    if limit == 0 || output.len() <= limit {
        return output.to_string();
    }

    if (tool_name == "run_command" || tool_name == "bash")
        && let Ok(mut val) = serde_json::from_str::<serde_json::Value>(output)
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
        "{}\n[... {omitted} chars omitted ...]\n{}",
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

#[async_trait::async_trait(?Send)]
impl acp::Agent for PrismAgent {
    async fn initialize(
        &self,
        _args: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::LATEST)
            .agent_capabilities(
                acp::AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(acp::PromptCapabilities::new()),
            )
            .agent_info(acp::Implementation::new("prism", env!("CARGO_PKG_VERSION"))))
    }

    async fn authenticate(
        &self,
        _args: acp::AuthenticateRequest,
    ) -> acp::Result<acp::AuthenticateResponse> {
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        args: acp::NewSessionRequest,
    ) -> acp::Result<acp::NewSessionResponse> {
        self.evict_sessions_if_needed();

        let session_id = Uuid::new_v4().to_string();
        let cwd = args.cwd;
        let messages = vec![self.build_system_message(Some(&cwd))];

        self.sessions.borrow_mut().insert(
            session_id.clone(),
            AcpSession {
                messages,
                cancelled: Arc::new(AtomicBool::new(false)),
                allowed_tools: HashSet::new(),
                model: None,
                cwd,
            },
        );

        Ok(acp::NewSessionResponse::new(session_id))
    }

    async fn prompt(
        &self,
        args: acp::PromptRequest,
    ) -> acp::Result<acp::PromptResponse> {
        let session_id = args.session_id.to_string();

        // Extract text from prompt content blocks
        let user_text: String = args
            .prompt
            .iter()
            .filter_map(|block| match block {
                acp::ContentBlock::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Reset cancellation flag and push user message
        {
            let mut sessions = self.sessions.borrow_mut();
            let session = sessions.get_mut(&session_id).ok_or_else(|| {
                acp::Error::invalid_params()
                    .data(json!(format!("session not found: {session_id}")))
            })?;
            session.cancelled.store(false, Ordering::Release);
            session.messages.push(Message {
                role: "user".into(),
                content: Some(json!(user_text)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            });
        }

        let stop_reason = self.run_agent_loop(&session_id).await;

        // Flush memory
        if let Err(e) = self.memory.borrow().flush() {
            tracing::warn!("memory flush failed: {e}");
        }

        Ok(acp::PromptResponse::new(stop_reason))
    }

    async fn cancel(&self, args: acp::CancelNotification) -> acp::Result<()> {
        let session_id = args.session_id.to_string();
        let sessions = self.sessions.borrow();
        if let Some(session) = sessions.get(&session_id) {
            session.cancelled.store(true, Ordering::Release);
        }
        Ok(())
    }

    async fn load_session(
        &self,
        args: acp::LoadSessionRequest,
    ) -> acp::Result<acp::LoadSessionResponse> {
        self.evict_sessions_if_needed();

        let session_id = args.session_id.to_string();

        let sessions_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".prism/sessions");

        let session = crate::session::Session::load_by_id_prefix(&sessions_dir, &session_id)
            .map_err(|e| acp::Error::invalid_params().data(json!(format!("{e}"))))?;

        // Rebuild messages: system prompt + saved conversation history
        let cwd = args.cwd;
        let mut messages = vec![self.build_system_message(Some(&cwd))];
        messages.extend(session.messages);
        Self::trim_messages(&mut messages, self.config.max_session_messages);

        self.sessions.borrow_mut().insert(
            session_id.clone(),
            AcpSession {
                messages,
                cancelled: Arc::new(AtomicBool::new(false)),
                allowed_tools: HashSet::new(),
                model: Some(session.model),
                cwd,
            },
        );

        Ok(acp::LoadSessionResponse::new())
    }

    async fn set_session_model(
        &self,
        args: acp::SetSessionModelRequest,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        let session_id = args.session_id.to_string();
        let model_id = args.model_id.to_string();

        let mut sessions = self.sessions.borrow_mut();
        if let Some(session) = sessions.get_mut(&session_id) {
            session.model = Some(model_id);
        }

        Ok(acp::SetSessionModelResponse::new())
    }
}

pub async fn run_acp_server(config: Config) -> Result<()> {
    let agent = Rc::new(PrismAgent::new(config));

    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
    let stdin = tokio::io::stdin().compat();
    let stdout = tokio::io::stdout().compat_write();

    let (connection, io_task) = acp::AgentSideConnection::new(
        agent.clone(),
        stdout,
        stdin,
        |fut| {
            tokio::task::spawn_local(fut);
        },
    );

    agent.set_connection(connection);

    io_task.await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_tool_output_within_limit() {
        let output = "short output";
        assert_eq!(truncate_tool_output("bash", output, 100), output);
    }

    #[test]
    fn test_truncate_tool_output_exceeds_limit() {
        let output = "a".repeat(300);
        let result = truncate_tool_output("read_file", &output, 100);
        assert!(result.len() < 300);
        assert!(result.contains("[..."));
        assert!(result.contains("chars omitted"));
    }

    #[test]
    fn test_truncate_tool_output_multibyte_utf8() {
        // Each emoji is 4 bytes
        let output = "🔥".repeat(100); // 400 bytes
        let result = truncate_tool_output("read_file", &output, 100);
        // Must not panic and must produce valid UTF-8
        assert!(result.len() < 400);
        assert!(result.contains("[..."));
    }

    #[test]
    fn test_truncate_bash_json_output() {
        let long_stdout = "x".repeat(500);
        let output = json!({"exit_code": 0, "stdout": long_stdout, "stderr": ""}).to_string();
        let result = truncate_tool_output("bash", &output, 200);
        assert!(result.len() < output.len());
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["stdout"].as_str().unwrap().contains("[..."));
    }

    #[test]
    fn test_truncate_empty_and_zero_limit() {
        assert_eq!(truncate_tool_output("bash", "", 100), "");
        let output = "some output";
        assert_eq!(truncate_tool_output("bash", output, 0), output);
    }

    #[test]
    fn test_snap_to_char_boundary_right() {
        let s = "hello 🌍 world";
        // 🌍 is 4 bytes, starts at byte 6
        // Trying to snap mid-emoji should advance to after it
        let boundary = snap_to_char_boundary_right(s, 7);
        assert!(s.is_char_boundary(boundary));
        assert!(boundary >= 7);

        // At a boundary already
        assert_eq!(snap_to_char_boundary_right(s, 0), 0);
        assert_eq!(snap_to_char_boundary_right(s, s.len()), s.len());

        // Past end
        assert_eq!(snap_to_char_boundary_right(s, s.len() + 10), s.len());
    }

    #[test]
    fn test_trim_messages_within_limit() {
        let mut msgs = vec![
            make_msg("system"),
            make_msg("user"),
            make_msg("assistant"),
        ];
        PrismAgent::trim_messages(&mut msgs, 10);
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn test_trim_messages_exceeds_limit() {
        let mut msgs: Vec<Message> = (0..20)
            .map(|i| make_msg(&format!("msg-{i}")))
            .collect();
        // First message is "system"
        msgs[0].role = "system".into();
        PrismAgent::trim_messages(&mut msgs, 5);
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].role, "system");
    }

    #[test]
    fn test_trim_messages_zero_limit_is_noop() {
        let mut msgs = vec![make_msg("system"), make_msg("user")];
        PrismAgent::trim_messages(&mut msgs, 0);
        assert_eq!(msgs.len(), 2);
    }

    fn make_msg(role: &str) -> Message {
        Message {
            role: role.into(),
            content: Some(json!("test")),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }
    }
}
