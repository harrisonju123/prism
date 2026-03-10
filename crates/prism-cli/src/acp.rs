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

use agent_client_protocol as acp;
use anyhow::Result;
use futures::StreamExt;
use prism_client::PrismClient;
use prism_types::{ChatCompletionRequest, Message, MessageRole};
use serde_json::json;
use uuid::Uuid;

use crate::common::{
    self, ToolCallBuilder, accumulate_tool_call_deltas, build_system_prompt,
    reconstruct_tool_calls, truncate_tool_output,
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
    pub fn new(config: Config, mcp_registry: Option<Arc<McpRegistry>>, skill_registry: SkillRegistry) -> Self {
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

    #[allow(clippy::await_holding_refcell_ref)]
    async fn build_system_message(&self, cwd: Option<&Path>) -> Message {
        let memory_content = self.memory.borrow().load().await;
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

        let mcp_section = self
            .mcp_registry
            .as_deref()
            .map(|r| r.system_prompt_section())
            .unwrap_or("");

        let full_system = build_system_prompt(
            self.config.model.system_prompt.as_deref(),
            &memory_content,
            &format!("{cwd_section}{skills_section}"),
            mcp_section,
        );

        common::system_message(full_system)
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

    /// Evict oldest sessions when we exceed max_sessions.
    fn evict_sessions_if_needed(&self) {
        let mut sessions = self.sessions.borrow_mut();
        let max = self.config.session.max_sessions;
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
        let client =
            PrismClient::new(&self.config.gateway.url).with_api_key(&self.config.gateway.api_key);
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
            let (cancelled, mut messages, session_model) = {
                let mut sessions = self.sessions.borrow_mut();
                match sessions.get_mut(session_id) {
                    Some(s) => (
                        s.cancelled.clone(),
                        std::mem::take(&mut s.messages),
                        s.model.clone(),
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

                        // Request permission for write tools
                        if !crate::permissions::is_read_only(name) {
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
                            _ => {
                                tools::dispatch(
                                    name,
                                    &args,
                                    &self.config,
                                    Some(&session_cwd),
                                    self.mcp_registry.as_deref(),
                                )
                                .await
                                .into_text()
                            }
                        };
                        let result =
                            truncate_tool_output(name, &result, self.config.model.max_tool_output);

                        // Post-tool-use hooks
                        let result = self.hook_runner.run_post_hooks(name, &args, &result).await;

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

                        // Inject skill content as a user message after the tool result
                        if let Some(content) = skill_injection {
                            messages.push(common::user_message(content));
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
        Some(BuiltinTool::ReadFile | BuiltinTool::ListDir) => acp::ToolKind::Read,
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
        let messages = vec![self.build_system_message(Some(&cwd)).await];

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
        let mut messages = vec![self.build_system_message(Some(&cwd)).await];
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

pub async fn run_acp_server(config: Config, mcp_registry: Option<Arc<McpRegistry>>, skill_registry: SkillRegistry) -> Result<()> {
    let agent = Rc::new(PrismAgent::new(config, mcp_registry, skill_registry));

    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
    let stdin = tokio::io::stdin().compat();
    let stdout = tokio::io::stdout().compat_write();

    let (connection, io_task) =
        acp::AgentSideConnection::new(agent.clone(), stdout, stdin, |fut| {
            tokio::task::spawn_local(fut);
        });

    agent.set_connection(connection);

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
