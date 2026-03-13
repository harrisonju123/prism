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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use agent_client_protocol as acp;
use anyhow::Result;
use futures::StreamExt;
use prism_client::{PrismClient, RetryConfig};
use prism_types::{ChatCompletionRequest, Message, MessageRole};
use serde_json::json;
use uuid::Uuid;

use crate::common::{
    self, ToolCallBuilder, accumulate_tool_call_deltas, additional_dirs_section,
    build_system_prompt, reconstruct_tool_calls, truncate_tool_output,
};
use crate::compression::{self, ContextCompressor};
use crate::config::Config;
use crate::hooks::HookRunner;
use crate::hooks::config::PreToolAction;
use crate::mcp::McpRegistry;
use crate::memory::MemoryManager;
use crate::skills::SkillRegistry;
use crate::tools::{self, BuiltinTool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPermissionOutcome {
    AllowOnce,
    AllowSession,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinishReason {
    Stop,
    ToolCalls,
}

impl FinishReason {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "stop" => Some(Self::Stop),
            "tool_calls" => Some(Self::ToolCalls),
            _ => {
                tracing::warn!(finish_reason = s, "unknown finish_reason");
                None
            }
        }
    }
}

struct AcpSession {
    messages: Vec<Message>,
    cancelled: Arc<AtomicBool>,
    /// Tools the user has granted "allow for session" permission
    allowed_tools: HashSet<String>,
    /// Per-session model override (set via `session/set_model`)
    model: Option<String>,
    /// Working directory for this session (from Zed's project root)
    cwd: PathBuf,
    /// When the session was created — used for deterministic LRU eviction
    created_at: Instant,
    /// Plan mode: when set, only this file may be written; bash/run_command are blocked.
    /// Activated mid-session via `/plan <file>`, deactivated with `/plan off`.
    plan_file: Option<String>,
    /// Directories added via `add_dir` — ephemeral, scoped to this session.
    additional_dirs: Vec<PathBuf>,
    /// Auto-approve mode: when true, write tools execute without prompting the user.
    /// Activated via `/auto`, cleared by `/default`.
    auto_approve: bool,
    /// Canonical path → mtime at last ReadFile/WriteFile/EditFile.
    /// Used to detect external modifications between read and edit.
    file_read_mtimes: HashMap<String, std::time::SystemTime>,
}

fn available_commands() -> Vec<acp::AvailableCommand> {
    vec![
        acp::AvailableCommand::new("help", "List available slash commands"),
        acp::AvailableCommand::new("status", "Show current mode, model, and context info"),
        acp::AvailableCommand::new(
            "auto",
            "Enable auto-approve mode — all tools run without prompts",
        ),
        acp::AvailableCommand::new(
            "default",
            "Restore default mode — write tools require approval",
        ),
        acp::AvailableCommand::new(
            "plan",
            "Restrict writes to a single file; bash/run_command blocked. Usage: /plan <file> or /plan off",
        )
        .input(acp::AvailableCommandInput::Unstructured(
            acp::UnstructuredCommandInput::new("<file> or off"),
        )),
        acp::AvailableCommand::new("model", "Switch model for this session")
            .input(acp::AvailableCommandInput::Unstructured(
                acp::UnstructuredCommandInput::new("<model-name>"),
            )),
        acp::AvailableCommand::new(
            "undo",
            "Remove the last user/assistant exchange from context",
        ),
    ]
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
    mcp_registry: Option<Arc<McpRegistry>>,
    skill_registry: SkillRegistry,
    hook_runner: HookRunner,
    compressor: Option<ContextCompressor>,
}

impl PrismAgent {
    pub fn new(
        config: Config,
        mcp_registry: Option<Arc<McpRegistry>>,
        skill_registry: SkillRegistry,
    ) -> Self {
        let memory = MemoryManager::new(None, None);
        let hook_runner = config.build_hook_runner();
        let compressor = config.build_compressor();
        Self {
            sessions: RefCell::new(HashMap::new()),
            connection: RefCell::new(None),
            config,
            memory: RefCell::new(memory),
            mcp_registry,
            skill_registry,
            hook_runner,
            compressor,
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
        let notification = acp::SessionNotification::new(acp::SessionId::new(session_id), update);
        if let Err(e) = acp::Client::session_notification(&*conn, notification).await {
            tracing::warn!("failed to send session update: {e:?}");
        }
    }

    async fn send_text_chunk(&self, session_id: &str, text: &str) {
        self.send_update(
            session_id,
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::from(
                text,
            ))),
        )
        .await;
    }

    /// Returns the effective model for a session, falling back to the config default.
    fn effective_model(&self, session_id: &str) -> String {
        self.sessions
            .borrow()
            .get(session_id)
            .and_then(|s| s.model.clone())
            .unwrap_or_else(|| self.config.model.model.clone())
    }

    async fn send_available_commands(&self, session_id: &str) {
        self.send_update(
            session_id,
            acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(
                available_commands(),
            )),
        )
        .await;
    }

    async fn build_system_message(
        &self,
        cwd: Option<&Path>,
        additional_dirs: &[PathBuf],
    ) -> Message {
        // Extract what load() needs synchronously so the RefCell borrow is dropped
        // before the first .await (holding a RefCell borrow across an await is UB
        // if anything else borrows memory on the same thread while suspended).
        let (store, workspace_id) = {
            let m = self.memory.borrow();
            (m.store().cloned(), m.workspace_id())
        };
        let memory_content = MemoryManager::load_from(store.as_deref(), workspace_id).await;
        let skills_section = self.skill_registry.system_prompt_section();
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
        let dirs_section = additional_dirs_section(additional_dirs);

        let mcp_section = self
            .mcp_registry
            .as_deref()
            .map(|r| r.system_prompt_section())
            .unwrap_or("");

        let full_system = build_system_prompt(
            self.config.model.system_prompt.as_deref(),
            &memory_content,
            &format!("{cwd_section}{dirs_section}{skills_section}"),
            mcp_section,
        );

        common::system_message(full_system)
    }

    /// Ask the client for permission before executing a write tool.
    async fn request_tool_permission(
        &self,
        session_id: &str,
        tool_name: &str,
        tool_call_id: &acp::ToolCallId,
        title: &str,
        args: &serde_json::Value,
    ) -> ToolPermissionOutcome {
        // Check if already allowed for this session
        {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(session_id)
                && session.allowed_tools.contains(tool_name)
            {
                return ToolPermissionOutcome::AllowSession;
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
                        ToolPermissionOutcome::AllowSession
                    } else if id == "allow-once" {
                        ToolPermissionOutcome::AllowOnce
                    } else {
                        ToolPermissionOutcome::Deny
                    }
                }
                acp::RequestPermissionOutcome::Cancelled => ToolPermissionOutcome::Deny,
                _ => ToolPermissionOutcome::Deny,
            },
            Err(e) => {
                tracing::warn!("permission request failed: {e:?}");
                ToolPermissionOutcome::Deny
            }
        }
    }

    /// Handle a bridge approval request from a CLI session by routing it through Zed's UI.
    pub async fn handle_bridge_approval(
        &self,
        req: crate::approval_bridge::ApprovalRequest,
    ) -> crate::approval_bridge::ApprovalResponse {
        use crate::approval_bridge::{ApprovalDecision, ApprovalResponse};

        // Lazily create a synthetic bridge session for CLI requests
        const BRIDGE_SESSION_ID: &str = "__cli_bridge__";
        self.sessions
            .borrow_mut()
            .entry(BRIDGE_SESSION_ID.to_string())
            .or_insert_with(|| AcpSession {
                messages: Vec::new(),
                cancelled: Arc::new(AtomicBool::new(false)),
                allowed_tools: HashSet::new(),
                model: None,
                cwd: std::env::current_dir().unwrap_or_default(),
                created_at: Instant::now(),
                plan_file: None,
                additional_dirs: Vec::new(),
                auto_approve: false,
                file_read_mtimes: HashMap::new(),
            });

        let tool_call_id = acp::ToolCallId::new("bridge_tc");

        let outcome = self
            .request_tool_permission(
                BRIDGE_SESSION_ID,
                &req.tool_name,
                &tool_call_id,
                &req.title,
                &req.args,
            )
            .await;

        let decision = match outcome {
            ToolPermissionOutcome::AllowOnce => ApprovalDecision::AllowOnce,
            ToolPermissionOutcome::AllowSession => ApprovalDecision::AllowSession,
            ToolPermissionOutcome::Deny => ApprovalDecision::Deny,
        };

        ApprovalResponse { decision }
    }

    /// Evict oldest sessions when we exceed max_sessions.
    fn evict_sessions_if_needed(&self) {
        let mut sessions = self.sessions.borrow_mut();
        let max = self.config.session.max_sessions;
        if max == 0 || sessions.len() <= max {
            return;
        }
        let excess = sessions.len() - max;
        // Sort by created_at ascending so we evict the oldest sessions first
        let mut ordered: Vec<(&String, Instant)> =
            sessions.iter().map(|(k, v)| (k, v.created_at)).collect();
        ordered.sort_by_key(|(_, t)| *t);
        let keys_to_remove: Vec<String> = ordered
            .into_iter()
            .take(excess)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys_to_remove {
            sessions.remove(&key);
        }
    }

    async fn run_agent_loop(&self, session_id: &str) -> acp::StopReason {
        let client = PrismClient::new(&self.config.gateway.url)
            .with_api_key(&self.config.gateway.api_key)
            .with_retry_config(RetryConfig::with_max_retries(
                self.config.gateway.max_retries,
            ));
        let model_fallback = self.config.model.model.clone();

        // cwd is set once at session creation and never changes
        let session_cwd = {
            let sessions = self.sessions.borrow();
            match sessions.get(session_id) {
                Some(s) => s.cwd.clone(),
                None => return acp::StopReason::EndTurn,
            }
        };

        let mut tool_call_counter: u32 = 0;
        let tool_defs = tools::all_tool_definitions(self.mcp_registry.as_deref());

        for _turn in 0..self.config.model.max_turns {
            // Take messages out of RefCell to avoid clone. We'll put them back after
            // the stream completes. Safe: single-threaded LocalSet, no concurrent access.
            let (cancelled, mut messages, session_model, plan_file, auto_approve) = {
                let mut sessions = self.sessions.borrow_mut();
                match sessions.get_mut(session_id) {
                    Some(s) => (
                        s.cancelled.clone(),
                        std::mem::take(&mut s.messages),
                        s.model.clone(),
                        s.plan_file.clone(),
                        s.auto_approve,
                    ),
                    None => return acp::StopReason::EndTurn,
                }
            };

            if cancelled.load(Ordering::Acquire) {
                // Put messages back before returning
                let mut sessions = self.sessions.borrow_mut();
                if let Some(session) = sessions.get_mut(session_id) {
                    session.messages = messages;
                }
                return acp::StopReason::Cancelled;
            }

            let req = ChatCompletionRequest {
                model: session_model.unwrap_or_else(|| model_fallback.clone()),
                messages: messages.clone(),
                tools: Some(tool_defs.clone()),
                tool_choice: Some(json!("auto")),
                ..Default::default()
            };

            // Helper macro: put messages back before early return
            macro_rules! put_back {
                ($msgs:expr) => {{
                    let mut sessions = self.sessions.borrow_mut();
                    if let Some(session) = sessions.get_mut(session_id) {
                        session.messages = $msgs;
                    }
                }};
            }

            let stream_result = client.stream_chat_completion(&req).await;
            let mut stream = match stream_result {
                Ok(s) => s,
                Err(e) => {
                    put_back!(messages);
                    self.send_text_chunk(session_id, &format!("\n[error] {e}"))
                        .await;
                    return acp::StopReason::EndTurn;
                }
            };

            let mut content_buf = String::new();
            let mut tc_builders: HashMap<usize, ToolCallBuilder> = HashMap::new();
            let mut finish_reason: Option<FinishReason> = None;

            while let Some(chunk_result) = stream.next().await {
                if cancelled.load(Ordering::Acquire) {
                    put_back!(messages);
                    return acp::StopReason::Cancelled;
                }

                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        put_back!(messages);
                        self.send_text_chunk(session_id, &format!("\n[stream error] {e}"))
                            .await;
                        return acp::StopReason::EndTurn;
                    }
                };

                if !chunk.delta.is_empty() {
                    self.send_text_chunk(session_id, &chunk.delta).await;
                    content_buf.push_str(&chunk.delta);
                }

                if let Some(tc_arr) = chunk.tool_calls.as_ref().and_then(|v| v.as_array()) {
                    accumulate_tool_call_deltas(tc_arr, &mut tc_builders);
                }

                if let Some(ref fr) = chunk.finish_reason {
                    finish_reason = FinishReason::from_str(fr);
                }
            }

            if cancelled.load(Ordering::Acquire) {
                put_back!(messages);
                return acp::StopReason::Cancelled;
            }

            let tool_calls_vec = reconstruct_tool_calls(&tc_builders);

            // Push assistant message to local vec (no RefCell borrow needed)
            messages.push(Message {
                role: MessageRole::Assistant,
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

            // Context compression (or FIFO fallback) — operates on local vec directly
            if let Some(ref compressor) = self.compressor {
                if compressor
                    .should_compress(messages.len(), self.config.session.max_session_messages)
                {
                    if let Some(compressed) = compressor
                        .compress(&client, &messages, self.config.session.max_session_messages)
                        .await
                    {
                        tracing::info!(
                            before = messages.len(),
                            after = compressed.len(),
                            "context compressed"
                        );
                        messages = compressed;
                    } else {
                        tracing::info!("compression failed, falling back to FIFO trim");
                        compression::trim_messages_fifo(
                            &mut messages,
                            self.config.session.max_session_messages,
                        );
                    }
                }
            } else {
                compression::trim_messages_fifo(
                    &mut messages,
                    self.config.session.max_session_messages,
                );
            }

            match finish_reason {
                Some(FinishReason::Stop) | None => {
                    put_back!(messages);
                    return acp::StopReason::EndTurn;
                }
                Some(FinishReason::ToolCalls) => {
                    let mut turn_files: Vec<String> = Vec::new();
                    for tc in tool_calls_vec.unwrap_or_default() {
                        if cancelled.load(Ordering::Acquire) {
                            put_back!(messages);
                            return acp::StopReason::Cancelled;
                        }

                        let id = tc["id"].as_str().unwrap_or("").to_string();
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let mut args: serde_json::Value = tc["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(json!({}));

                        // Pre-tool-use hooks (run before permission check)
                        match self.hook_runner.run_pre_hooks(name, &args).await {
                            PreToolAction::Deny { message } => {
                                tracing::info!(tool = name, "hook denied: {message}");
                                messages.push(Message {
                                    role: MessageRole::Tool,
                                    content: Some(json!(format!("Hook denied: {message}"))),
                                    name: None,
                                    tool_calls: None,
                                    tool_call_id: Some(id),
                                    extra: Default::default(),
                                });
                                continue;
                            }
                            PreToolAction::Modify { args: new_args } => {
                                args = new_args;
                            }
                            PreToolAction::Allow => {}
                        }

                        tool_call_counter += 1;
                        let tool_call_id = acp::ToolCallId::new(format!("tc_{tool_call_counter}"));

                        let bt = BuiltinTool::from_str(name);
                        let kind = tool_kind(bt);
                        let title = tool_title(bt, name, &args);

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

                        // Enforce plan_file restrictions: block bash/run_command and any write
                        // to a file other than the designated plan file.
                        if let Some(ref pf) = plan_file {
                            let is_blocked =
                                matches!(bt, Some(BuiltinTool::Bash | BuiltinTool::RunCommand))
                                    || (!crate::permissions::is_read_only(name)
                                        && args["path"].as_str().unwrap_or("") != pf.as_str());
                            if is_blocked {
                                let reason = format!(
                                    "Blocked by plan mode — only `{pf}` may be written and bash/run_command are disabled."
                                );
                                self.send_update(
                                    session_id,
                                    acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                                        tool_call_id,
                                        acp::ToolCallUpdateFields::new()
                                            .status(acp::ToolCallStatus::Failed)
                                            .content(vec![
                                                acp::ContentBlock::from(reason.as_str()).into(),
                                            ]),
                                    )),
                                )
                                .await;
                                messages.push(Message {
                                    role: MessageRole::Tool,
                                    content: Some(json!(reason)),
                                    name: None,
                                    tool_calls: None,
                                    tool_call_id: Some(id),
                                    extra: Default::default(),
                                });
                                continue;
                            }
                        }

                        // Request permission for write tools (skip in auto mode)
                        if !crate::permissions::is_read_only(name) && !auto_approve {
                            let outcome = self
                                .request_tool_permission(
                                    session_id,
                                    name,
                                    &tool_call_id,
                                    &title,
                                    &args,
                                )
                                .await;
                            if matches!(outcome, ToolPermissionOutcome::Deny) {
                                // Emit rejection as a failed tool call
                                self.send_update(
                                    session_id,
                                    acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                                        tool_call_id,
                                        acp::ToolCallUpdateFields::new()
                                            .status(acp::ToolCallStatus::Failed)
                                            .content(vec![
                                                acp::ContentBlock::from(
                                                    "Permission denied by user".to_string(),
                                                )
                                                .into(),
                                            ]),
                                    )),
                                )
                                .await;

                                messages.push(Message {
                                    role: MessageRole::Tool,
                                    content: Some(json!(crate::permissions::PERMISSION_DENIED_MSG)),
                                    name: None,
                                    tool_calls: None,
                                    tool_call_id: Some(id),
                                    extra: Default::default(),
                                });
                                continue;
                            }
                        }

                        // Auto-save: flush IDE buffers to disk before file operations
                        if let Err(msg) = self.hook_runner.run_auto_save(&name, &args).await {
                            messages.push(Message {
                                role: MessageRole::Tool,
                                content: Some(json!(msg)),
                                name: None,
                                tool_calls: None,
                                tool_call_id: Some(id),
                                extra: Default::default(),
                            });
                            continue;
                        }

                        // Detect external file modifications since last read.
                        // Stores (path, is_write) so we can decide post-tool whether to warn.
                        let staleness_info: Option<(String, bool)> =
                            if matches!(bt, Some(BuiltinTool::EditFile | BuiltinTool::WriteFile)) {
                                if let Some(path) = args["path"].as_str() {
                                    let stored = {
                                        let sessions = self.sessions.borrow();
                                        sessions
                                            .get(session_id)
                                            .and_then(|s| s.file_read_mtimes.get(path).copied())
                                    };
                                    if let Some(stored_mtime) = stored {
                                        if let Ok(meta) = tokio::fs::metadata(path).await {
                                            if let Ok(current_mtime) = meta.modified() {
                                                if current_mtime != stored_mtime {
                                                    Some((
                                                        path.to_owned(),
                                                        matches!(bt, Some(BuiltinTool::WriteFile)),
                                                    ))
                                                } else {
                                                    None
                                                }
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                        let mut skill_injection: Option<String> = None;
                        let result = match bt {
                            Some(BuiltinTool::SaveMemory) => {
                                let key = args["key"].as_str().unwrap_or("note").to_string();
                                let value = args["value"].as_str().unwrap_or("").to_string();
                                self.memory.borrow_mut().append(key.clone(), value);
                                json!({"saved": true, "key": key}).to_string()
                            }
                            Some(BuiltinTool::Skill) => {
                                let exec = self.skill_registry.execute(
                                    args["name"].as_str().unwrap_or(""),
                                    args["args"].as_str().unwrap_or(""),
                                );
                                skill_injection = exec.injection;
                                exec.tool_result
                            }
                            Some(BuiltinTool::AddDir) => {
                                let path_str = args["path"].as_str().unwrap_or("").to_string();
                                let path = PathBuf::from(&path_str);
                                if !path.is_absolute() {
                                    json!({"error": "path must be absolute"}).to_string()
                                } else if !path.is_dir() {
                                    json!({"error": format!("not a directory: {path_str}")})
                                        .to_string()
                                } else {
                                    // Add dir (dedup) and snapshot the updated list in one borrow.
                                    let dirs_snapshot = {
                                        let mut sessions = self.sessions.borrow_mut();
                                        if let Some(session) = sessions.get_mut(session_id) {
                                            if !session.additional_dirs.contains(&path) {
                                                session.additional_dirs.push(path);
                                            }
                                            session.additional_dirs.clone()
                                        } else {
                                            Vec::new()
                                        }
                                    };
                                    // Rebuild system message (messages[0] is still in local vec)
                                    let new_sys = self
                                        .build_system_message(Some(&session_cwd), &dirs_snapshot)
                                        .await;
                                    if !messages.is_empty() {
                                        messages[0] = new_sys;
                                    }
                                    tools::dispatch(
                                        BuiltinTool::ListDir.as_str(),
                                        &json!({"path": path_str}),
                                        &self.config,
                                        Some(&session_cwd),
                                        None,
                                        &[],
                                    )
                                    .await
                                    .into_text()
                                }
                            }
                            _ => {
                                let additional_dirs = self
                                    .sessions
                                    .borrow()
                                    .get(session_id)
                                    .map(|s| s.additional_dirs.clone())
                                    .unwrap_or_default();
                                tools::dispatch(
                                    name,
                                    &args,
                                    &self.config,
                                    Some(&session_cwd),
                                    self.mcp_registry.as_deref(),
                                    &additional_dirs,
                                )
                                .await
                                .into_text()
                            }
                        };
                        let result =
                            truncate_tool_output(name, &result, self.config.model.max_tool_output);

                        // Post-tool-use hooks
                        let result = self.hook_runner.run_post_hooks(name, &args, &result).await;

                        // Update stored mtime after read/write/edit so own-writes don't trigger false positives
                        if matches!(
                            bt,
                            Some(
                                BuiltinTool::ReadFile
                                    | BuiltinTool::WriteFile
                                    | BuiltinTool::EditFile
                            )
                        ) {
                            if let Some(path) = args["path"].as_str() {
                                if let Ok(meta) = tokio::fs::metadata(path).await {
                                    if let Ok(mtime) = meta.modified() {
                                        let mut sessions = self.sessions.borrow_mut();
                                        if let Some(session) = sessions.get_mut(session_id) {
                                            session.file_read_mtimes.insert(path.to_owned(), mtime);
                                        }
                                    }
                                }
                            }
                        }

                        // Apply staleness warning based on tool outcome:
                        // - WriteFile: always warn (overwriting stale content is dangerous)
                        // - EditFile success: suppress — the edit landed fine, no re-read needed
                        // - EditFile failure + stale: embed fresh content so agent can retry inline
                        let result = if let Some((stale_path, is_write)) = staleness_info {
                            if is_write {
                                format!(
                                    "Warning: `{stale_path}` was modified externally since last read. Re-read the file first to avoid overwriting changes.\n\n{result}"
                                )
                            } else if result.starts_with("edited ") {
                                result
                            } else {
                                let fresh = tokio::fs::read_to_string(&stale_path)
                                    .await
                                    .unwrap_or_else(|e| format!("(could not read file: {e})"));
                                format!(
                                    "Warning: `{stale_path}` was modified since your last read. Your edit did not apply. Current file contents:\n\n{fresh}"
                                )
                            }
                        } else {
                            result
                        };

                        // Push tool result to local messages vec
                        messages.push(Message {
                            role: MessageRole::Tool,
                            content: Some(json!(&result)),
                            name: None,
                            tool_calls: None,
                            tool_call_id: Some(id),
                            extra: Default::default(),
                        });

                        // Emit ToolCallUpdate (completed)
                        self.send_update(
                            session_id,
                            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                                tool_call_id,
                                acp::ToolCallUpdateFields::new()
                                    .status(acp::ToolCallStatus::Completed)
                                    .content(vec![acp::ContentBlock::from(result).into()]),
                            )),
                        )
                        .await;

                        // Track files mutated by write/edit tools (for compile check)
                        if matches!(
                            BuiltinTool::from_str(name),
                            Some(BuiltinTool::WriteFile | BuiltinTool::EditFile)
                        ) {
                            if let Some(path) = args["path"].as_str() {
                                turn_files.push(path.to_owned());
                            }
                        }

                        // Inject skill content as a user message after the tool result
                        if let Some(content) = skill_injection {
                            messages.push(common::user_message(content));
                        }
                    }

                    // Post-turn compile validation
                    if let Some(ref cmd) = self.config.session.compile_check_command {
                        if let Some(msg) = crate::compile_check::run_compile_check(
                            &turn_files,
                            cmd,
                            self.config.session.compile_check_timeout,
                            None,
                        )
                        .await
                        {
                            tracing::info!(compile_check = %msg, "post-turn compile check");
                            // Surface to user via ACP before injecting into LLM context
                            self.send_text_chunk(session_id, &format!("\n{msg}")).await;
                            messages.push(common::user_message(msg));
                        }
                    }
                }
            }

            // Put messages back into RefCell at end of each turn
            put_back!(messages);
        }

        // Exceeded max turns
        acp::StopReason::MaxTurnRequests
    }
}

fn tool_kind(bt: Option<BuiltinTool>) -> acp::ToolKind {
    match bt {
        Some(BuiltinTool::ReadFile | BuiltinTool::ListDir | BuiltinTool::AddDir) => {
            acp::ToolKind::Read
        }
        Some(BuiltinTool::WriteFile | BuiltinTool::EditFile) => acp::ToolKind::Edit,
        Some(BuiltinTool::GlobFiles | BuiltinTool::GrepFiles) => acp::ToolKind::Search,
        Some(BuiltinTool::Bash | BuiltinTool::RunCommand) => acp::ToolKind::Execute,
        Some(BuiltinTool::WebFetch) => acp::ToolKind::Fetch,
        _ => acp::ToolKind::Other,
    }
}

fn tool_title(bt: Option<BuiltinTool>, name: &str, args: &serde_json::Value) -> String {
    match bt {
        Some(BuiltinTool::ReadFile) => format!("Read {}", args["path"].as_str().unwrap_or("file")),
        Some(BuiltinTool::WriteFile) => {
            format!("Write {}", args["path"].as_str().unwrap_or("file"))
        }
        Some(BuiltinTool::EditFile) => format!("Edit {}", args["path"].as_str().unwrap_or("file")),
        Some(BuiltinTool::ListDir) => format!("List {}", args["path"].as_str().unwrap_or(".")),
        Some(BuiltinTool::Bash) => {
            let preview =
                common::truncate_with_ellipsis(args["command"].as_str().unwrap_or("command"), 60);
            format!("Run: {preview}")
        }
        Some(BuiltinTool::RunCommand) => {
            format!("Run: {}", args["command"].as_str().unwrap_or("command"))
        }
        Some(BuiltinTool::GlobFiles) => {
            format!("Glob {}", args["pattern"].as_str().unwrap_or("*"))
        }
        Some(BuiltinTool::GrepFiles) => {
            format!("Grep {}", args["pattern"].as_str().unwrap_or("pattern"))
        }
        Some(BuiltinTool::WebFetch) => format!("Fetch {}", args["url"].as_str().unwrap_or("url")),
        Some(BuiltinTool::SaveMemory) => {
            format!("Save memory: {}", args["key"].as_str().unwrap_or("note"))
        }
        Some(BuiltinTool::SpawnAgent) => {
            format!(
                "Spawn: {}",
                common::truncate_with_ellipsis(args["task"].as_str().unwrap_or("task"), 60)
            )
        }
        Some(BuiltinTool::Recall) => {
            format!("Recall: {}", args["query"].as_str().unwrap_or("memory"))
        }
        Some(BuiltinTool::Skill) => {
            format!("Skill: {}", args["name"].as_str().unwrap_or("skill"))
        }
        Some(BuiltinTool::CheckBackgroundTasks) => "Check background tasks".to_string(),
        Some(BuiltinTool::RecordDecision) => {
            format!("Decide: {}", args["title"].as_str().unwrap_or("decision"))
        }
        Some(BuiltinTool::AddDir) => {
            format!("Add dir {}", args["path"].as_str().unwrap_or("(unknown)"))
        }
        Some(BuiltinTool::AskHuman) => {
            format!(
                "Ask human: {}",
                common::truncate_with_ellipsis(args["question"].as_str().unwrap_or("question"), 60)
            )
        }
        Some(BuiltinTool::ReportBlocker) => {
            format!("Report blocker: {}", args["title"].as_str().unwrap_or("(no title)"))
        }
        None => name.to_string(),
    }
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
        let messages = vec![self.build_system_message(Some(&cwd), &[]).await];

        self.sessions.borrow_mut().insert(
            session_id.clone(),
            AcpSession {
                messages,
                cancelled: Arc::new(AtomicBool::new(false)),
                allowed_tools: HashSet::new(),
                model: None,
                cwd,
                created_at: Instant::now(),
                plan_file: None,
                additional_dirs: Vec::new(),
                auto_approve: false,
                file_read_mtimes: HashMap::new(),
            },
        );

        self.send_available_commands(&session_id).await;

        Ok(acp::NewSessionResponse::new(session_id))
    }

    // Safe: single-threaded LocalSet, no concurrent RefCell borrow possible
    #[allow(clippy::await_holding_refcell_ref)]
    async fn prompt(&self, args: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
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

        let cmd = user_text.trim();

        // /help
        if cmd == "/help" {
            let help = "**Available commands:**\n\n\
                **Session:**\n\
                - `/auto` — all tools run without prompts\n\
                - `/default` — write tools require approval (default)\n\
                - `/status` — show mode, model, and context size\n\n\
                **Model:**\n\
                - `/model <name>` — switch model for this session\n\n\
                **Context:**\n\
                - `/plan <file>` — restrict writes to one file; bash blocked\n\
                - `/plan off` — deactivate plan mode\n\
                - `/undo` — remove last user/assistant exchange from context\n\n\
                **Help:**\n\
                - `/help` — show this message";
            self.send_text_chunk(&session_id, help).await;
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }

        // /auto
        if cmd == "/auto" {
            let mut sessions = self.sessions.borrow_mut();
            if let Some(s) = sessions.get_mut(&session_id) {
                s.auto_approve = true;
            }
            self.send_text_chunk(
                &session_id,
                "Auto mode enabled — all tools run without prompts.",
            )
            .await;
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }

        // /default
        if cmd == "/default" {
            let mut sessions = self.sessions.borrow_mut();
            if let Some(s) = sessions.get_mut(&session_id) {
                s.auto_approve = false;
                s.allowed_tools.clear();
            }
            self.send_text_chunk(
                &session_id,
                "Default mode restored — write tools require approval.",
            )
            .await;
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }

        // /status
        if cmd == "/status" {
            let status = {
                let sessions = self.sessions.borrow();
                if let Some(s) = sessions.get(&session_id) {
                    let mode = match (&s.plan_file, s.auto_approve) {
                        (_, true) => "auto".to_string(),
                        (Some(f), _) => format!("plan ({})", f),
                        _ => "default".to_string(),
                    };
                    let model = s
                        .model
                        .clone()
                        .unwrap_or_else(|| self.config.model.model.clone());
                    // messages[0] is always the system prompt, so subtract 1 for user/assistant
                    let exchanges = s.messages.len().saturating_sub(1);
                    format!(
                        "**Mode:** {mode}\n**Model:** {model}\n**Messages in context:** {exchanges}"
                    )
                } else {
                    "Session not found.".to_string()
                }
            };
            self.send_text_chunk(&session_id, &status).await;
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }

        // /model — must be "/model" exactly or "/model <name>" (not "/modelX")
        if cmd == "/model" || cmd.starts_with("/model ") {
            let new_model = cmd.strip_prefix("/model").unwrap_or("").trim();
            if new_model.is_empty() {
                let current = self.effective_model(&session_id);
                self.send_text_chunk(
                    &session_id,
                    &format!("Current model: `{current}`. Usage: `/model <name>`"),
                )
                .await;
            } else {
                let old = {
                    let mut sessions = self.sessions.borrow_mut();
                    if let Some(s) = sessions.get_mut(&session_id) {
                        let old = s
                            .model
                            .clone()
                            .unwrap_or_else(|| self.config.model.model.clone());
                        s.model = Some(new_model.to_string());
                        old
                    } else {
                        self.config.model.model.clone()
                    }
                };
                self.send_text_chunk(
                    &session_id,
                    &format!("Model switched: `{old}` → `{new_model}`"),
                )
                .await;
            }
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }

        // /undo — remove the last user/assistant exchange from context
        if cmd == "/undo" {
            let removed = {
                let mut sessions = self.sessions.borrow_mut();
                if let Some(s) = sessions.get_mut(&session_id) {
                    // Find the last user message (messages[0] is always the system prompt)
                    let last_user = s.messages.iter().rposition(|m| m.role == MessageRole::User);
                    match last_user {
                        Some(idx) if idx > 0 => {
                            let before = s.messages.len();
                            s.messages.truncate(idx);
                            before - idx
                        }
                        _ => 0,
                    }
                } else {
                    0
                }
            };
            let msg = if removed > 0 {
                format!("Removed {removed} message(s) from context.")
            } else {
                "Nothing to undo.".to_string()
            };
            self.send_text_chunk(&session_id, &msg).await;
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }

        // Handle /plan slash command — activates or deactivates plan mode without an LLM call.
        // Usage:  /plan PLAN.md   → only PLAN.md may be written; bash/run_command blocked
        //         /plan off       → deactivate plan mode
        // Must be "/plan" exactly or "/plan <file>" (not "/plank" etc.)
        if cmd == "/plan" || cmd.starts_with("/plan ") {
            let arg = cmd.strip_prefix("/plan").unwrap_or("").trim();
            let response = if arg.is_empty() || arg == "off" {
                let mut sessions = self.sessions.borrow_mut();
                if let Some(s) = sessions.get_mut(&session_id) {
                    s.plan_file = None;
                }
                "Plan mode deactivated — all tools available.".to_string()
            } else {
                let mut sessions = self.sessions.borrow_mut();
                if let Some(s) = sessions.get_mut(&session_id) {
                    s.plan_file = Some(arg.to_string());
                }
                format!(
                    "Plan mode active. Only `{arg}` may be written; bash and run_command are blocked."
                )
            };
            self.send_text_chunk(&session_id, &response).await;
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }

        // Reset cancellation flag and push user message
        {
            let mut sessions = self.sessions.borrow_mut();
            let session = sessions.get_mut(&session_id).ok_or_else(|| {
                acp::Error::invalid_params().data(json!(format!("session not found: {session_id}")))
            })?;
            session.cancelled.store(false, Ordering::Release);
            session.messages.push(Message {
                role: MessageRole::User,
                content: Some(json!(user_text)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            });
        }

        let stop_reason = self.run_agent_loop(&session_id).await;

        // Flush memory
        if let Err(e) = self.memory.borrow_mut().flush().await {
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

        let sessions_dir = crate::config::prism_home().join("sessions");

        let session = crate::session::Session::load_by_id_prefix(&sessions_dir, &session_id)
            .map_err(|e| acp::Error::invalid_params().data(json!(format!("{e}"))))?;

        // Rebuild messages: system prompt + saved conversation history
        let cwd = args.cwd;
        let mut messages = vec![self.build_system_message(Some(&cwd), &[]).await];
        messages.extend(session.messages);
        compression::trim_messages_fifo(&mut messages, self.config.session.max_session_messages);

        self.sessions.borrow_mut().insert(
            session_id.clone(),
            AcpSession {
                messages,
                cancelled: Arc::new(AtomicBool::new(false)),
                allowed_tools: HashSet::new(),
                model: Some(session.model),
                cwd,
                created_at: Instant::now(),
                plan_file: None,
                additional_dirs: Vec::new(),
                auto_approve: false,
                file_read_mtimes: HashMap::new(),
            },
        );

        self.send_available_commands(&session_id).await;

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

pub async fn run_acp_server(
    config: Config,
    mcp_registry: Option<Arc<McpRegistry>>,
    skill_registry: SkillRegistry,
) -> Result<()> {
    let agent = Rc::new(PrismAgent::new(config, mcp_registry, skill_registry));

    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
    let stdin = tokio::io::stdin().compat();
    let stdout = tokio::io::stdout().compat_write();

    let (connection, io_task) =
        acp::AgentSideConnection::new(agent.clone(), stdout, stdin, |fut| {
            tokio::task::spawn_local(fut);
        });

    agent.set_connection(connection);

    // Spawn approval bridge listener so CLI sessions can route prompts through Zed
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Ok(listener) = crate::approval_bridge::ApprovalListener::bind(&cwd) {
        let agent_clone = agent.clone();
        tokio::task::spawn_local(async move {
            listener
                .serve(|req| agent_clone.handle_bridge_approval(req))
                .await;
        });
    }

    io_task.await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::truncate_tool_output;

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
        let output = "🔥".repeat(100);
        let result = truncate_tool_output("read_file", &output, 100);
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
    fn test_trim_messages_within_limit() {
        let mut msgs = vec![
            make_msg(MessageRole::System),
            make_msg(MessageRole::User),
            make_msg(MessageRole::Assistant),
        ];
        compression::trim_messages_fifo(&mut msgs, 10);
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn test_trim_messages_exceeds_limit() {
        let mut msgs: Vec<Message> = (0..20).map(|_| make_msg(MessageRole::User)).collect();
        msgs[0].role = MessageRole::System;
        compression::trim_messages_fifo(&mut msgs, 5);
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].role, MessageRole::System);
    }

    #[test]
    fn test_trim_messages_zero_limit_is_noop() {
        let mut msgs = vec![make_msg(MessageRole::System), make_msg(MessageRole::User)];
        compression::trim_messages_fifo(&mut msgs, 0);
        assert_eq!(msgs.len(), 2);
    }

    fn make_msg(role: MessageRole) -> Message {
        Message {
            role,
            content: Some(json!("test")),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }
    }
}
