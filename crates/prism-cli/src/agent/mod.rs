pub mod spawn;

use anyhow::{Result, anyhow};
use chrono::Utc;
use futures::StreamExt;
use prism_client::PrismClient;
use prism_types::{ChatCompletionRequest, Message, MessageRole};
use serde_json::json;
use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::permissions::{self, PermissionDecision, ToolPermissionGate};
use crate::session::Session;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentStopReason {
    Stop,
    CostCap,
    Interrupt,
    MaxTurns,
}

impl AgentStopReason {
    fn to_session_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::CostCap => "cost_cap",
            Self::Interrupt => "interrupt",
            Self::MaxTurns => "max_turns",
        }
    }
}

pub struct Agent {
    client: PrismClient,
    config: Config,
    session: Session,
    memory: MemoryManager,
    mcp_registry: Option<McpRegistry>,
    permission_gate: ToolPermissionGate,
    hook_runner: HookRunner,
    compressor: Option<ContextCompressor>,
}

impl Agent {
    pub fn new(
        client: PrismClient,
        config: Config,
        task: &str,
        mcp_registry: Option<McpRegistry>,
        memory: MemoryManager,
    ) -> Self {
        let episode_id = Uuid::new_v4();
        let session = Session::new(episode_id, task, &config.model.model);
        let permission_gate = ToolPermissionGate::resolve(config.extensions.permission_mode);
        let hook_runner = config.build_hook_runner();
        let compressor = config.build_compressor();
        Self {
            client,
            config,
            session,
            memory,
            mcp_registry,
            permission_gate,
            hook_runner,
            compressor,
        }
    }

    pub fn from_session(
        client: PrismClient,
        config: Config,
        session: Session,
        mcp_registry: Option<McpRegistry>,
        memory: MemoryManager,
    ) -> Self {
        let permission_gate = ToolPermissionGate::resolve(config.extensions.permission_mode);
        let hook_runner = config.build_hook_runner();
        let compressor = config.build_compressor();
        Self {
            client,
            config,
            session,
            memory,
            mcp_registry,
            permission_gate,
            hook_runner,
            compressor,
        }
    }

    pub async fn run(&mut self, task: &str) -> Result<()> {
        tracing::info!(episode_id = %self.session.episode_id, model = %self.config.model.model, "starting agent session");

        let memory_content = self.memory.load().await;
        let mcp_section = self
            .mcp_registry
            .as_ref()
            .map(|r| r.system_prompt_section())
            .unwrap_or("");

        let cwd = std::env::current_dir().unwrap_or_default();
        let instructions_section = crate::instructions::load_project_instructions(&cwd);
        let git_section = crate::git::gather_git_context(&cwd);
        let full_system = build_system_prompt(
            self.config.model.system_prompt.as_deref(),
            &memory_content,
            &format!("{instructions_section}{git_section}"),
            mcp_section,
        );

        self.session
            .messages
            .push(common::system_message(full_system));
        self.session.messages.push(Message {
            role: MessageRole::User,
            content: Some(json!(task)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        });

        self.inner_run().await
    }

    pub async fn resume(&mut self, task: &str) -> Result<()> {
        tracing::info!(
            episode_id = %self.session.episode_id,
            model = %self.config.model.model,
            turns_so_far = %self.session.turns,
            "resuming agent session"
        );

        if !task.is_empty() {
            self.session.messages.push(Message {
                role: MessageRole::User,
                content: Some(json!(task)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            });
        }

        self.inner_run().await
    }

    async fn inner_run(&mut self) -> Result<()> {
        let interrupted = Arc::new(AtomicBool::new(false));
        let flag = interrupted.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            flag.store(true, Ordering::SeqCst);
        });

        let mut total_prompt: u32 = 0;
        let mut total_completion: u32 = 0;
        let mut total_cost_usd: f64 = 0.0;
        let mut turns: u32 = 0;
        let model_name = self.config.model.model.clone();
        let mut stop_reason: Option<AgentStopReason> = None;
        let tool_defs = tools::all_tool_definitions(self.mcp_registry.as_ref());

        for _turn in 0..self.config.model.max_turns {
            if interrupted.load(Ordering::Relaxed) {
                eprintln!("\n[interrupt] Ctrl+C — stopping");
                stop_reason = Some(AgentStopReason::Interrupt);
                break;
            }

            let req = ChatCompletionRequest {
                model: self.config.model.model.clone(),
                messages: self.session.messages.clone(),
                tools: Some(tool_defs.clone()),
                tool_choice: Some(json!("auto")),
                ..Default::default()
            };

            let mut stream = self
                .client
                .stream_chat_completion(&req)
                .await
                .map_err(|e| anyhow!("stream_chat_completion failed: {e}"))?;

            turns += 1;

            let mut content_buf = String::new();
            let mut tc_builders: HashMap<usize, ToolCallBuilder> = HashMap::new();
            let mut finish_reason: Option<FinishReason> = None;

            while let Some(chunk_result) = stream.next().await {
                if interrupted.load(Ordering::Relaxed) {
                    eprintln!("\n[interrupt] Ctrl+C — stopping");
                    stop_reason = Some(AgentStopReason::Interrupt);
                    break;
                }

                let chunk = chunk_result.map_err(|e| anyhow!("stream error: {e}"))?;

                if !chunk.delta.is_empty() {
                    print!("{}", chunk.delta);
                    let _ = std::io::stdout().flush();
                    content_buf.push_str(&chunk.delta);
                }

                if let Some(tc_arr) = chunk
                    .tool_calls
                    .as_ref()
                    .and_then(|v: &serde_json::Value| v.as_array())
                {
                    accumulate_tool_call_deltas(tc_arr, &mut tc_builders);
                }

                if let Some(ref fr) = chunk.finish_reason {
                    finish_reason = FinishReason::from_str(fr);
                }

                if let Some(u) = &chunk.usage {
                    total_prompt += u.prompt_tokens;
                    total_completion += u.completion_tokens;

                    let (in_rate, out_rate): (f64, f64) = match model_name.as_str() {
                        m if m.contains("claude-opus-4") => (5.50, 27.50),
                        m if m.contains("claude-sonnet-4") => (3.30, 16.50),
                        m if m.contains("claude-haiku-4") => (1.00, 5.00),
                        m if m.contains("gpt-5") && m.contains("codex") => (1.75, 14.0),
                        m if m.contains("gpt-5") => (1.75, 14.0),
                        m if m.contains("gpt-4o-mini") => (0.15, 0.6),
                        m if m.contains("gpt-4o") => (2.5, 10.0),
                        m if m.contains("qwen3") => (0.15, 1.20),
                        m if m.contains("kimi") => (0.60, 3.00),
                        m if m.contains("gpt-oss") => (0.15, 0.60),
                        m if m.contains("minimax") => (0.30, 1.20),
                        m if m.contains("gemma") => (0.23, 0.38),
                        m if m.contains("ministral") => (0.20, 0.20),
                        m if m.contains("nova") => (0.33, 2.75),
                        m if m.contains("gemini-1.5-pro") => (1.25, 5.0),
                        m if m.contains("gemini-1.5-flash") => (0.075, 0.3),
                        _ => (0.0, 0.0),
                    };
                    let turn_cost = (u.prompt_tokens as f64 * in_rate
                        + u.completion_tokens as f64 * out_rate)
                        / 1_000_000.0;
                    total_cost_usd += turn_cost;
                }
            }

            if stop_reason == Some(AgentStopReason::Interrupt) {
                break;
            }

            let tool_calls_vec = reconstruct_tool_calls(&tc_builders);

            // Push assistant message
            self.session.messages.push(Message {
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

            // Save after each turn
            self.session.turns = turns;
            self.session.updated_at = chrono::Utc::now().to_rfc3339();
            if let Err(e) = self.session.save(&self.config.session.sessions_dir) {
                tracing::warn!("failed to save session: {e}");
            }

            // Context compression (or FIFO fallback)
            if let Some(ref compressor) = self.compressor
                && compressor.should_compress(
                    self.session.messages.len(),
                    self.config.session.max_session_messages,
                )
            {
                if let Some(compressed) = compressor
                    .compress(
                        &self.client,
                        &self.session.messages,
                        self.config.session.max_session_messages,
                    )
                    .await
                {
                    tracing::info!(
                        before = self.session.messages.len(),
                        after = compressed.len(),
                        "context compressed"
                    );
                    self.session.messages = compressed;
                } else {
                    tracing::info!("compression failed, falling back to FIFO trim");
                    compression::trim_messages_fifo(
                        &mut self.session.messages,
                        self.config.session.max_session_messages,
                    );
                }
            }

            // Check cost cap
            if let Some(cap) = self.config.model.max_cost_usd
                && total_cost_usd >= cap
            {
                eprintln!(
                    "\n[cost-cap] ${:.4} >= cap ${:.4} — stopping",
                    total_cost_usd, cap
                );
                stop_reason = Some(AgentStopReason::CostCap);
                break;
            }

            match finish_reason {
                Some(FinishReason::Stop) | None => {
                    if !content_buf.is_empty() && !content_buf.ends_with('\n') {
                        println!();
                    }
                    stop_reason = Some(AgentStopReason::Stop);
                    break;
                }
                Some(FinishReason::ToolCalls) => {
                    for tc in tool_calls_vec.unwrap_or_default() {
                        let id = tc["id"].as_str().unwrap_or("").to_string();
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let mut args: serde_json::Value = tc["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(json!({}));

                        // Pre-tool-use hooks (run before permission check)
                        match self.hook_runner.run_pre_hooks(name, &args).await {
                            PreToolAction::Deny { message } => {
                                eprintln!("[hook] {name}  denied: {message}");
                                self.session.messages.push(Message {
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

                        let args_preview = common::truncate_with_ellipsis(&args.to_string(), 120);
                        eprintln!("[tool] {name}  args={args_preview}");

                        // Permission check — blocks on TTY read when prompting,
                        // which is intentional (we're waiting for user input)
                        let decision = self.permission_gate.check_permission(name, &args);

                        if decision == PermissionDecision::Deny {
                            eprintln!("[tool] {name}  permission denied");
                            self.session.messages.push(Message {
                                role: MessageRole::Tool,
                                content: Some(json!(permissions::PERMISSION_DENIED_MSG)),
                                name: None,
                                tool_calls: None,
                                tool_call_id: Some(id),
                                extra: Default::default(),
                            });
                            continue;
                        }

                        let t0 = std::time::Instant::now();
                        let result = match BuiltinTool::from_str(name) {
                            Some(BuiltinTool::SaveMemory) => {
                                let key = args["key"].as_str().unwrap_or("note").to_string();
                                let value = args["value"].as_str().unwrap_or("").to_string();
                                match self.memory.save(&key, &value).await {
                                    Ok(_) => {
                                        serde_json::json!({"saved": true, "key": key}).to_string()
                                    }
                                    Err(e) => {
                                        // Fall back to in-memory buffer
                                        self.memory.append(key.clone(), value);
                                        serde_json::json!({"saved": true, "key": key, "note": format!("buffered: {e}")}).to_string()
                                    }
                                }
                            }
                            Some(BuiltinTool::Recall) => self.handle_recall(&args).await,
                            Some(BuiltinTool::SpawnAgent) => {
                                let task = args["task"].as_str().unwrap_or("").to_string();
                                let model = args["model"].as_str().map(str::to_string);
                                let cost_cap = args["cost_cap"].as_f64();
                                let timeout_secs = args["timeout_secs"].as_u64();
                                let spawn_config = spawn::SpawnConfig {
                                    task,
                                    model,
                                    cost_cap,
                                    tools: None,
                                    timeout_secs,
                                };
                                match spawn::spawn_agent(
                                    spawn_config,
                                    &self.config.gateway.url,
                                    &self.config.gateway.api_key,
                                )
                                .await
                                {
                                    Ok(r) => serde_json::to_string(&r)
                                        .unwrap_or_else(|_| r.summary.clone()),
                                    Err(e) => {
                                        format!("{{\"status\":\"error\",\"summary\":\"{e}\"}}")
                                    }
                                }
                            }
                            _ => {
                                tools::dispatch(name, &args, None, self.mcp_registry.as_ref()).await
                            }
                        };
                        let result =
                            truncate_tool_output(name, &result, self.config.model.max_tool_output);

                        // Post-tool-use hooks
                        let result = self.hook_runner.run_post_hooks(name, &args, &result).await;

                        let elapsed_ms = t0.elapsed().as_millis();

                        let result_preview =
                            common::truncate_with_ellipsis(result.trim_start(), 80);
                        eprintln!(
                            "[tool] {name}  {}ms  {} bytes  {result_preview}",
                            elapsed_ms,
                            result.len()
                        );

                        self.session.messages.push(Message {
                            role: MessageRole::Tool,
                            content: Some(json!(result)),
                            name: None,
                            tool_calls: None,
                            tool_call_id: Some(id),
                            extra: Default::default(),
                        });
                    }
                }
            }
        }

        // Flush any pending memory entries
        if let Err(e) = self.memory.flush().await {
            tracing::warn!("memory flush failed: {e}");
        }

        let cost_str = if total_cost_usd > 0.0 {
            format!("  ~${:.4}", total_cost_usd)
        } else {
            String::new()
        };
        eprintln!(
            "[session] {}  {} turns  {} in / {} out tokens{}",
            model_name, turns, total_prompt, total_completion, cost_str
        );
        eprintln!(
            "[session] episode {}",
            &self.session.episode_id.to_string()[..8]
        );

        // Final session update
        self.session.turns = turns;
        self.session.total_prompt_tokens = total_prompt;
        self.session.total_completion_tokens = total_completion;
        self.session.total_cost_usd = total_cost_usd;
        self.session.stop_reason = stop_reason.map(|r| r.to_session_str().to_string());
        self.session.updated_at = chrono::Utc::now().to_rfc3339();
        if let Err(e) = self.session.save(&self.config.session.sessions_dir) {
            tracing::warn!("failed to save session: {e}");
        }

        if stop_reason.is_none() {
            self.session.stop_reason = Some(AgentStopReason::MaxTurns.to_session_str().to_string());
            if let Err(e) = self.session.save(&self.config.session.sessions_dir) {
                tracing::warn!("failed to save session: {e}");
            }
            anyhow::bail!("exceeded max_turns ({})", self.config.model.max_turns);
        }
        Ok(())
    }

    /// Handle the `recall` tool — loads context from uglyhat store.
    async fn handle_recall(&self, args: &serde_json::Value) -> String {
        use uglyhat::store::Store;

        let Some(store) = self.memory.store() else {
            return json!({"error": "no uglyhat context store available"}).to_string();
        };
        let Some(ws_id) = self.memory.workspace_id() else {
            return json!({"error": "no workspace configured"}).to_string();
        };

        if let Some(thread_name) = args["thread"].as_str() {
            match store.recall_thread(ws_id, thread_name).await {
                Ok(ctx) => serde_json::to_string(&ctx)
                    .unwrap_or_else(|e| json!({"error": format!("serialize: {e}")}).to_string()),
                Err(e) => json!({"error": e.to_string()}).to_string(),
            }
        } else {
            let tags: Vec<String> = args["tags"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();

            let since = args["since"]
                .as_str()
                .and_then(|s| parse_duration_str(s).ok().map(|d| Utc::now() - d));

            match store.recall_by_tags(ws_id, tags, since).await {
                Ok(result) => serde_json::to_string(&result)
                    .unwrap_or_else(|e| json!({"error": format!("serialize: {e}")}).to_string()),
                Err(e) => json!({"error": e.to_string()}).to_string(),
            }
        }
    }
}

fn parse_duration_str(s: &str) -> Result<chrono::Duration> {
    uglyhat::util::parse_duration(s).map_err(|e| anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let input = serde_json::json!({
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
}
