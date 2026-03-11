pub mod background;
pub mod spawn;

use anyhow::{Result, anyhow};
use chrono::Utc;
use futures::StreamExt;
use prism_client::PrismClient;
use prism_types::{ChatCompletionRequest, Message, MessageRole};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tokio::task::JoinSet;
use uuid::Uuid;

use background::BackgroundTaskManager;

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
use crate::permissions::{
    self, PermissionDecision, PermissionMode, ToolPermissionGate, is_read_only,
};
use crate::session::Session;
use crate::skills::SkillRegistry;
use crate::tools::{self, BuiltinTool};

/// Strip cwd prefix to produce a relative path; falls back to absolute if outside cwd.
fn normalize_path(path: &str) -> String {
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(rel) = std::path::Path::new(path).strip_prefix(&cwd)
    {
        return rel.to_string_lossy().into_owned();
    }
    path.to_string()
}

/// Returns true if the user's prompt contains signals that a decision is required
/// before implementation (e.g. "decide between", "option A vs B", "trade-offs").
fn has_decision_signals(text: &str) -> bool {
    let lower = text.to_lowercase();
    const SIGNALS: &[&str] = &[
        "decide between",
        "choose between",
        "option a",
        "option b",
        "before implementation",
        "before implementing",
        "trade-off",
        "tradeoff",
        "compare approaches",
        "which approach",
        "pros and cons",
        "evaluate alternatives",
    ];
    SIGNALS.iter().any(|s| lower.contains(s))
}

const DECISION_CHECKPOINT_THRESHOLD_DEFAULT: u32 = 8;
const QUESTION_CHECKPOINT_INTERVAL_DEFAULT: u32 = 5;

fn decision_checkpoint_threshold() -> u32 {
    std::env::var("PRISM_DECISION_CHECKPOINT_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DECISION_CHECKPOINT_THRESHOLD_DEFAULT)
}

fn question_checkpoint_interval() -> u32 {
    std::env::var("PRISM_QUESTION_CHECKPOINT_INTERVAL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(QUESTION_CHECKPOINT_INTERVAL_DEFAULT)
}

/// Tracks consecutive read-only turns and emits a convergence nudge when the threshold is reached.
struct ExplorationBudget {
    consecutive_readonly_turns: u32,
    threshold: u32,
    /// True after the nudge has fired for the current streak; clears on streak reset.
    streak_nudged: bool,
}

impl ExplorationBudget {
    fn new(threshold: u32) -> Self {
        Self {
            consecutive_readonly_turns: 0,
            threshold,
            streak_nudged: false,
        }
    }

    /// Record a turn. Returns `Some(nudge message)` if the threshold is reached for the first time
    /// in the current streak. Resets when a write tool is used or the model produces substantive text.
    fn record_turn(&mut self, all_readonly: bool, had_substantive_text: bool) -> Option<String> {
        if self.threshold == 0 {
            return None; // disabled
        }

        if all_readonly && !had_substantive_text {
            self.consecutive_readonly_turns += 1;
        } else {
            self.consecutive_readonly_turns = 0;
            self.streak_nudged = false;
        }

        if self.consecutive_readonly_turns >= self.threshold && !self.streak_nudged {
            self.streak_nudged = true;
            Some(format!(
                "[System] You have made {} consecutive exploration turns without proposing an approach. \
                 Summarize what you have learned so far and either propose a specific implementation plan \
                 or explain what specific information is still missing.",
                self.threshold
            ))
        } else {
            None
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentStopReason {
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

struct PreparedToolCall {
    index: usize,
    id: String,
    name: String,
    args: serde_json::Value,
}

enum ToolOutcome {
    Result {
        index: usize,
        id: String,
        name: String,
        args: serde_json::Value,
        result: String,
        elapsed_ms: u128,
    },
    Denied {
        index: usize,
        id: String,
        message: String,
    },
}

impl ToolOutcome {
    fn index(&self) -> usize {
        match self {
            Self::Result { index, .. } => *index,
            Self::Denied { index, .. } => *index,
        }
    }
}

pub struct Agent {
    client: PrismClient,
    config: Config,
    session: Session,
    memory: MemoryManager,
    mcp_registry: Option<Arc<McpRegistry>>,
    skill_registry: SkillRegistry,
    permission_gate: ToolPermissionGate,
    hook_runner: HookRunner,
    compressor: Option<ContextCompressor>,
    background: BackgroundTaskManager,
    // Shared interrupt flag — set by ctrl-c handler (owned by REPL) or per-run handler
    pub interrupted: Arc<AtomicBool>,
    // Counts ctrl-c presses within a single run; shared with the spawned signal task.
    // Kept as a field so two-press kill logic persists correctly across REPL turns.
    interrupt_count: Arc<AtomicU32>,
    /// Current thread name (from UH_THREAD env or handoff)
    current_thread: Option<String>,
    /// Count of write-tool denials while in plan mode
    plan_mode_violations: u32,
    /// Plan file path for structural plan mode enforcement (from config)
    plan_file: Option<String>,
    /// True when setup_plan_mode_guardrails auto-created the current thread
    plan_thread_auto_created: bool,
    /// Paths mutated by write_file / edit_file during this session
    files_touched: HashSet<String>,
    /// Cancels the previous run's ctrl-c listener task when a new run starts.
    cancel_interrupt: Arc<AtomicBool>,
    /// Directories added via `add_dir` — ephemeral, scoped to this session.
    additional_dirs: Vec<std::path::PathBuf>,
    /// True when the user's prompt contains decision-required signals.
    decision_checkpoint_armed: bool,
    /// Cumulative count of read-only exploration tool calls in current run.
    exploration_count: u32,
    /// True after the decision checkpoint nudge has fired (one-shot per run).
    decision_checkpoint_fired: bool,
    /// Turn number at which the last question checkpoint fired (0 = never).
    last_question_checkpoint_turn: u32,
}

const UH_AGENT_NAME_ENV: &str = "UH_AGENT_NAME";
const UH_AGENT_NAME_DEFAULT: &str = "claude";
const UH_THREAD_ENV: &str = "UH_THREAD";

impl Agent {
    pub fn new(
        client: PrismClient,
        config: Config,
        task: &str,
        mcp_registry: Option<Arc<McpRegistry>>,
        memory: MemoryManager,
        skill_registry: SkillRegistry,
    ) -> Self {
        let episode_id = Uuid::new_v4();
        let session = Session::new(episode_id, task, &config.model.model);
        let permission_gate = ToolPermissionGate::resolve(config.extensions.permission_mode);
        let hook_runner = config.build_hook_runner();
        let compressor = config.build_compressor();
        let current_thread = std::env::var(UH_THREAD_ENV).ok();
        let plan_file = config.extensions.plan_file.clone();
        Self {
            client,
            config,
            session,
            memory,
            mcp_registry,
            skill_registry,
            permission_gate,
            hook_runner,
            compressor,
            background: BackgroundTaskManager::new(),
            interrupted: Arc::new(AtomicBool::new(false)),
            interrupt_count: Arc::new(AtomicU32::new(0)),
            current_thread,
            plan_mode_violations: 0,
            plan_file,
            plan_thread_auto_created: false,
            files_touched: HashSet::new(),
            cancel_interrupt: Arc::new(AtomicBool::new(false)),
            additional_dirs: Vec::new(),
            decision_checkpoint_armed: false,
            exploration_count: 0,
            decision_checkpoint_fired: false,
            last_question_checkpoint_turn: 0,
        }
    }

    pub fn from_session(
        client: PrismClient,
        config: Config,
        session: Session,
        mcp_registry: Option<Arc<McpRegistry>>,
        memory: MemoryManager,
        skill_registry: SkillRegistry,
    ) -> Self {
        let permission_gate = ToolPermissionGate::resolve(config.extensions.permission_mode);
        let hook_runner = config.build_hook_runner();
        let compressor = config.build_compressor();
        let current_thread = std::env::var(UH_THREAD_ENV).ok();
        let plan_file = config.extensions.plan_file.clone();
        Self {
            client,
            config,
            session,
            memory,
            mcp_registry,
            skill_registry,
            permission_gate,
            hook_runner,
            compressor,
            background: BackgroundTaskManager::new(),
            interrupted: Arc::new(AtomicBool::new(false)),
            interrupt_count: Arc::new(AtomicU32::new(0)),
            current_thread,
            plan_mode_violations: 0,
            plan_file,
            plan_thread_auto_created: false,
            files_touched: HashSet::new(),
            cancel_interrupt: Arc::new(AtomicBool::new(false)),
            additional_dirs: Vec::new(),
            decision_checkpoint_armed: false,
            exploration_count: 0,
            decision_checkpoint_fired: false,
            last_question_checkpoint_turn: 0,
        }
    }

    async fn build_full_system_prompt(&mut self) -> String {
        let memory_content = self.memory.load().await;
        let mcp_section = self
            .mcp_registry
            .as_deref()
            .map(|r| r.system_prompt_section())
            .unwrap_or("");

        let cwd = std::env::current_dir().unwrap_or_default();
        let instructions_section = crate::instructions::load_project_instructions(&cwd);
        let git_section = crate::git::gather_git_context(&cwd);
        let skills_section = self.skill_registry.system_prompt_section();
        let dirs_section = additional_dirs_section(&self.additional_dirs);
        build_system_prompt(
            self.config.model.system_prompt.as_deref(),
            &memory_content,
            &format!("{instructions_section}{git_section}{dirs_section}{skills_section}"),
            mcp_section,
        )
    }

    /// Reset conversation to just the system message (rebuilds system prompt to pick up latest context).
    pub async fn clear_conversation(&mut self) {
        let full_system = self.build_full_system_prompt().await;
        self.session
            .set_active_messages(vec![common::system_message(full_system)]);
        self.session.updated_at = chrono::Utc::now().to_rfc3339();
        if let Err(e) = self.session.save(&self.config.session.sessions_dir) {
            tracing::warn!("failed to save session after clear: {e}");
        }
    }

    /// Compress the conversation context. Falls back to FIFO trim if LLM compression unavailable.
    pub async fn compact(&mut self) {
        let active = self.session.active_messages();
        let max = self.config.session.max_session_messages;

        if let Some(ref compressor) = self.compressor {
            if let Some(compressed) = compressor.compress(&self.client, &active, max).await {
                eprintln!(
                    "[compact] compressed {} → {} messages",
                    active.len(),
                    compressed.len()
                );
                self.session.set_active_messages(compressed);
            } else {
                eprintln!("[compact] LLM compression failed, falling back to FIFO trim");
                let mut msgs = active;
                compression::trim_messages_fifo(&mut msgs, max);
                eprintln!("[compact] trimmed to {} messages", msgs.len());
                self.session.set_active_messages(msgs);
            }
        } else {
            let mut msgs = active;
            compression::trim_messages_fifo(&mut msgs, max);
            eprintln!(
                "[compact] trimmed to {} messages (no compression model configured)",
                msgs.len()
            );
            self.session.set_active_messages(msgs);
        }

        self.session.updated_at = chrono::Utc::now().to_rfc3339();
        if let Err(e) = self.session.save(&self.config.session.sessions_dir) {
            tracing::warn!("failed to save session after compact: {e}");
        }
    }

    /// Poll for newly completed background tasks and return notification strings.
    /// Does NOT consume them — they remain buffered for injection at next turn start.
    pub fn poll_background_notifications(&mut self) -> Vec<String> {
        self.background
            .poll_completed()
            .iter()
            .map(|t| {
                format!(
                    "[bg] task {} \"{}\" completed in {:.1}s — {}",
                    t.task_id, t.description, t.elapsed_secs, t.result.summary,
                )
            })
            .collect()
    }

    pub async fn run(&mut self, task: &str) -> Result<()> {
        tracing::info!(episode_id = %self.session.episode_id, model = %self.config.model.model, "starting agent session");

        // Activate structural plan mode if a plan file is configured
        if self.permission_gate.mode() == PermissionMode::Plan && self.plan_file.is_some() {
            let activated = self.setup_plan_mode_guardrails().await;
            self.permission_gate.set_plan_file_enforcement(activated);
        }

        let full_system = self.build_full_system_prompt().await;

        self.session
            .push_message(common::system_message(full_system));
        self.session.push_message(Message {
            role: MessageRole::User,
            content: Some(json!(task)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        });
        self.decision_checkpoint_armed = has_decision_signals(task);

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
            self.session.push_message(Message {
                role: MessageRole::User,
                content: Some(json!(task)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            });
            self.decision_checkpoint_armed = has_decision_signals(task);
        }

        self.inner_run().await
    }

    async fn inner_run(&mut self) -> Result<()> {
        let result = self.inner_run_impl().await;
        // Always tear down plan mode guardrails, even on early return or error.
        if self.permission_gate.mode() == PermissionMode::Plan && self.plan_file.is_some() {
            self.teardown_plan_mode_guardrails().await;
        }
        result
    }

    async fn inner_run_impl(&mut self) -> Result<()> {
        // Reset per-run state
        self.interrupted.store(false, Ordering::SeqCst);
        self.plan_mode_violations = 0;
        self.interrupt_count.store(0, Ordering::SeqCst);
        self.exploration_count = 0;
        self.decision_checkpoint_fired = false;
        self.last_question_checkpoint_turn = 0;
        // Signal the previous run's ctrl-c task to exit, then create a fresh token for this run.
        self.cancel_interrupt.store(true, Ordering::SeqCst);
        self.cancel_interrupt = Arc::new(AtomicBool::new(false));
        let flag_count = self.interrupt_count.clone();
        let flag_interrupted = self.interrupted.clone();
        let cancel = self.cancel_interrupt.clone();
        tokio::spawn(async move {
            loop {
                let _ = tokio::signal::ctrl_c().await;
                // Exit if this run has already finished (REPL moved on to next turn).
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let prev = flag_count.fetch_add(1, Ordering::SeqCst);
                flag_interrupted.store(true, Ordering::SeqCst);
                if prev == 0 {
                    eprintln!(
                        "\n\x1b[33m[interrupt] Finishing current turn — press Ctrl+C again to force quit\x1b[0m"
                    );
                } else {
                    eprintln!("\n\x1b[31m[interrupt] Force quit\x1b[0m");
                    std::process::exit(130);
                }
            }
        });

        // Accumulate on top of values already recorded (supports multi-resume)
        let mut total_prompt: u32 = self.session.total_prompt_tokens;
        let mut total_completion: u32 = self.session.total_completion_tokens;
        let mut total_cost_usd: f64 = self.session.total_cost_usd;
        let mut turns: u32 = self.session.turns;
        let model_name = self.config.model.model.clone();
        let mut stop_reason: Option<AgentStopReason> = None;
        let tool_defs = tools::all_tool_definitions(self.mcp_registry.as_deref());
        let mut exploration_budget =
            ExplorationBudget::new(self.config.model.exploration_nudge_turns);
        let decision_threshold = decision_checkpoint_threshold();
        let q_interval = question_checkpoint_interval();

        for _turn in 0..self.config.model.max_turns {
            if self.interrupted.load(Ordering::Relaxed) {
                self.permission_gate.renderer().interrupt_notice();
                stop_reason = Some(AgentStopReason::Interrupt);
                break;
            }

            // Heartbeat + state transition to Working
            self.send_heartbeat_and_set_working().await;

            // Poll for workspace-scoped decisions from other agents
            self.poll_and_inject_decisions().await;

            // --- Exploration checkpoints ---
            let current_turn = turns;

            // A) Decision checkpoint: fire once after enough exploration calls
            if self.decision_checkpoint_armed
                && !self.decision_checkpoint_fired
                && self.exploration_count >= decision_checkpoint_threshold()
            {
                self.decision_checkpoint_fired = true;
                let nudge = "[Decision Checkpoint] You have explored enough context. \
The user asked you to decide between approaches before implementing. \
STOP exploring and present your findings now:\n\
1. List the options you've identified (Option A, Option B, etc.)\n\
2. For each option, state trade-offs (pros/cons)\n\
3. Give your recommendation with rationale\n\
4. Ask the user any clarifying questions that would affect the choice\n\
5. Use `record_decision` to persist your recommendation once confirmed\n\
6. Wait for user confirmation before implementing";
                self.session
                    .push_message(common::user_message(nudge.to_string()));
                tracing::info!(
                    exploration_count = self.exploration_count,
                    "decision checkpoint fired"
                );
            }

            // B) Question checkpoint: every N turns, remind agent to surface unknowns
            let q_interval = question_checkpoint_interval();
            if q_interval > 0
                && current_turn > 0
                && current_turn % q_interval == 0
                && current_turn != self.last_question_checkpoint_turn
            {
                self.last_question_checkpoint_turn = current_turn;
                let nudge = "[Question Checkpoint] You've been working for several turns. \
Before continuing, consider:\n\
- Are there ambiguities in the requirements you should ask about?\n\
- Are you making assumptions the user should confirm?\n\
- Is this heading in the direction the user expects?\n\
If you have questions, ask them now. If you're confident, continue.";
                self.session
                    .push_message(common::user_message(nudge.to_string()));
                tracing::info!(turn = current_turn, "question checkpoint fired");
            }

            // Inject completed background tasks as user messages
            let completed = self.background.take_pending();
            for task in &completed {
                self.permission_gate.renderer().background_task_complete(
                    &task.task_id,
                    &task.description,
                    task.elapsed_secs,
                );
                if task.result.cost.is_finite() {
                    total_cost_usd += task.result.cost;
                }
                let notification = format!(
                    "[Background task completed] task_id={} description=\"{}\" elapsed={:.1}s\n\
                     Result: {}",
                    task.task_id,
                    task.description,
                    task.elapsed_secs,
                    serde_json::to_string(&task.result)
                        .unwrap_or_else(|_| task.result.summary.clone()),
                );
                self.session
                    .push_message(common::user_message(notification));
            }

            let req = ChatCompletionRequest {
                model: self.config.model.model.clone(),
                messages: self.session.active_messages(),
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

            // Per-turn counters for cost display
            let mut turn_prompt: u32 = 0;
            let mut turn_completion: u32 = 0;
            let mut turn_cost: f64 = 0.0;

            let mut content_buf = String::new();
            let mut tc_builders: HashMap<usize, ToolCallBuilder> = HashMap::new();
            let mut finish_reason: Option<FinishReason> = None;

            while let Some(chunk_result) = stream.next().await {
                if self.interrupted.load(Ordering::Relaxed) {
                    self.permission_gate.renderer().interrupt_notice();
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
                    turn_prompt = u.prompt_tokens;
                    turn_completion = u.completion_tokens;
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
                    turn_cost = (u.prompt_tokens as f64 * in_rate
                        + u.completion_tokens as f64 * out_rate)
                        / 1_000_000.0;
                    total_cost_usd += turn_cost;
                }
            }

            // Print per-turn cost summary
            if self.config.session.show_cost && (turn_prompt > 0 || turn_completion > 0) {
                eprintln!(
                    "\x1b[2m[turn {} · {} in / {} out · ~${:.4} · session ${:.4}]\x1b[0m",
                    turns, turn_prompt, turn_completion, turn_cost, total_cost_usd
                );
            }

            if stop_reason == Some(AgentStopReason::Interrupt) {
                break;
            }

            let tool_calls_vec = reconstruct_tool_calls(&tc_builders);

            // Push assistant message
            self.session.push_message(Message {
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
            let active = self.session.active_messages();
            if let Some(ref compressor) = self.compressor
                && compressor
                    .should_compress(active.len(), self.config.session.max_session_messages)
            {
                if let Some(compressed) = compressor
                    .compress(
                        &self.client,
                        &active,
                        self.config.session.max_session_messages,
                    )
                    .await
                {
                    tracing::info!(
                        before = active.len(),
                        after = compressed.len(),
                        "context compressed"
                    );
                    self.session.set_active_messages(compressed);
                } else {
                    tracing::info!("compression failed, falling back to FIFO trim");
                    let mut msgs = active;
                    compression::trim_messages_fifo(
                        &mut msgs,
                        self.config.session.max_session_messages,
                    );
                    self.session.set_active_messages(msgs);
                }
            }

            // Check cost cap
            if let Some(cap) = self.config.model.max_cost_usd
                && total_cost_usd >= cap
            {
                self.permission_gate
                    .renderer()
                    .cost_cap_notice(total_cost_usd, cap);
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
                    let raw_tool_calls = tool_calls_vec.unwrap_or_default();

                    // --- Phase 1: Sequential gate — pre-hooks + permission checks ---
                    let mut prepared: Vec<PreparedToolCall> = Vec::new();
                    let mut outcomes: Vec<ToolOutcome> = Vec::new();

                    for (index, tc) in raw_tool_calls.iter().enumerate() {
                        let id = tc["id"].as_str().unwrap_or("").to_string();
                        let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                        let mut args: serde_json::Value = tc["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(json!({}));

                        match self.hook_runner.run_pre_hooks(&name, &args).await {
                            PreToolAction::Deny { message } => {
                                self.permission_gate.renderer().hook_denied(&name, &message);
                                outcomes.push(ToolOutcome::Denied {
                                    index,
                                    id,
                                    message: format!("Hook denied: {message}"),
                                });
                                continue;
                            }
                            PreToolAction::Modify { args: new_args } => {
                                args = new_args;
                            }
                            PreToolAction::Allow => {}
                        }

                        let args_preview = common::truncate_with_ellipsis(&args.to_string(), 120);
                        self.permission_gate
                            .renderer()
                            .tool_start(&name, &args_preview);

                        // Permission check — blocks on TTY read when prompting,
                        // which is intentional (we're waiting for user input)
                        let decision = self.permission_gate.check_permission(&name, &args);

                        if decision == PermissionDecision::Deny {
                            if self.permission_gate.mode() == PermissionMode::Plan
                                && !is_read_only(&name)
                            {
                                self.plan_mode_violations += 1;
                            }
                            self.permission_gate.renderer().tool_denied(&name);
                            outcomes.push(ToolOutcome::Denied {
                                index,
                                id,
                                message: permissions::PERMISSION_DENIED_MSG.to_string(),
                            });
                            continue;
                        }

                        // Guardrail check: enforce thread-scoped restrictions
                        if let Some(denial) = self.check_guardrails(&name, &args).await {
                            self.permission_gate.renderer().tool_denied(&name);
                            outcomes.push(ToolOutcome::Denied {
                                index,
                                id,
                                message: denial,
                            });
                            continue;
                        }

                        prepared.push(PreparedToolCall {
                            index,
                            id,
                            name,
                            args,
                        });
                    }

                    // --- Phase 2: Parallel execution ---
                    // Agent-context tools (SaveMemory, Recall, Skill) run inline since they need
                    // &mut self. Everything else spawns into a JoinSet.
                    let mcp_arc = self.mcp_registry.clone();
                    // Arc<str> so each spawned task gets a cheap pointer clone, not a heap alloc
                    let gateway_url: Arc<str> = Arc::from(self.config.gateway.url.as_str());
                    let gateway_key: Arc<str> = Arc::from(self.config.gateway.api_key.as_str());
                    let max_tool_output = self.config.model.max_tool_output;
                    // Clone config for sandbox checks inside spawned tasks
                    let task_config = self.config.clone();
                    // Capture cwd once so spawned tasks can resolve relative paths correctly.
                    let run_cwd: std::path::PathBuf = std::env::current_dir().unwrap_or_default();

                    let mut joinset: JoinSet<ToolOutcome> = JoinSet::new();
                    // Track (index, id) for every call spawned into the JoinSet so we can
                    // synthesize error outcomes for any tasks that panic (their ID would
                    // otherwise be lost, leaving the model without a tool response).
                    let mut joinset_pending: Vec<(usize, String)> = Vec::new();

                    let mut pending_skill_messages: Vec<String> = Vec::new();

                    for ptc in prepared {
                        match BuiltinTool::from_str(&ptc.name) {
                            Some(BuiltinTool::Skill) => {
                                let t0 = std::time::Instant::now();
                                let exec = self.skill_registry.execute(
                                    ptc.args["name"].as_str().unwrap_or(""),
                                    ptc.args["args"].as_str().unwrap_or(""),
                                );
                                if let Some(content) = exec.injection {
                                    pending_skill_messages.push(content);
                                }
                                outcomes.push(ToolOutcome::Result {
                                    index: ptc.index,
                                    id: ptc.id,
                                    name: ptc.name,
                                    args: ptc.args,
                                    result: exec.tool_result,
                                    elapsed_ms: t0.elapsed().as_millis(),
                                });
                            }
                            Some(BuiltinTool::SaveMemory) => {
                                let key = ptc.args["key"].as_str().unwrap_or("note").to_string();
                                let value = ptc.args["value"].as_str().unwrap_or("").to_string();
                                let t0 = std::time::Instant::now();
                                let result = match self.memory.save(&key, &value).await {
                                    Ok(_) => {
                                        serde_json::json!({"saved": true, "key": key}).to_string()
                                    }
                                    Err(e) => {
                                        self.memory.append(key.clone(), value);
                                        serde_json::json!({"saved": true, "key": key, "note": format!("buffered: {e}")}).to_string()
                                    }
                                };
                                outcomes.push(ToolOutcome::Result {
                                    index: ptc.index,
                                    id: ptc.id,
                                    name: ptc.name,
                                    args: ptc.args,
                                    result,
                                    elapsed_ms: t0.elapsed().as_millis(),
                                });
                            }
                            Some(BuiltinTool::Recall) => {
                                let t0 = std::time::Instant::now();
                                let result = self.handle_recall(&ptc.args).await;
                                outcomes.push(ToolOutcome::Result {
                                    index: ptc.index,
                                    id: ptc.id,
                                    name: ptc.name,
                                    args: ptc.args,
                                    result,
                                    elapsed_ms: t0.elapsed().as_millis(),
                                });
                            }
                            Some(BuiltinTool::RecordDecision) => {
                                let t0 = std::time::Instant::now();
                                let result = self.handle_record_decision(&ptc.args).await;
                                outcomes.push(ToolOutcome::Result {
                                    index: ptc.index,
                                    id: ptc.id,
                                    name: ptc.name,
                                    args: ptc.args,
                                    result,
                                    elapsed_ms: t0.elapsed().as_millis(),
                                });
                            }
                            Some(BuiltinTool::AddDir) => {
                                let t0 = std::time::Instant::now();
                                let path_str = ptc.args["path"].as_str().unwrap_or("").to_string();
                                let path = std::path::PathBuf::from(&path_str);
                                let result = if !path.is_absolute() {
                                    serde_json::json!({"error": "path must be absolute"})
                                        .to_string()
                                } else if !path.is_dir() {
                                    serde_json::json!({"error": format!("not a directory: {path_str}")}).to_string()
                                } else {
                                    if !self.additional_dirs.contains(&path) {
                                        self.additional_dirs.push(path);
                                    }
                                    // Rebuild system prompt so the LLM sees the new dir
                                    let new_sys = self.build_full_system_prompt().await;
                                    let active = self.session.active_messages();
                                    if !active.is_empty() {
                                        let mut msgs = active;
                                        msgs[0] = common::system_message(new_sys);
                                        self.session.set_active_messages(msgs);
                                    }
                                    tools::dispatch(
                                        BuiltinTool::ListDir.as_str(),
                                        &serde_json::json!({"path": path_str}),
                                        &self.config,
                                        None,
                                        None,
                                    )
                                    .await
                                    .into_text()
                                };
                                outcomes.push(ToolOutcome::Result {
                                    index: ptc.index,
                                    id: ptc.id,
                                    name: ptc.name,
                                    args: ptc.args,
                                    result,
                                    elapsed_ms: t0.elapsed().as_millis(),
                                });
                            }
                            Some(BuiltinTool::CheckBackgroundTasks) => {
                                let t0 = std::time::Instant::now();
                                let completed = self.background.take_pending();
                                // Accumulate costs from completed tasks
                                for task in &completed {
                                    if task.result.cost.is_finite() {
                                        total_cost_usd += task.result.cost;
                                    }
                                }
                                let active: Vec<serde_json::Value> = self
                                    .background
                                    .active_tasks()
                                    .iter()
                                    .map(|t| {
                                        json!({
                                            "task_id": t.task_id,
                                            "description": t.description,
                                            "running_secs": t.started_at.elapsed().as_secs_f64(),
                                        })
                                    })
                                    .collect();
                                let completed_json: Vec<serde_json::Value> = completed.iter().map(|t| {
                                    json!({
                                        "task_id": t.task_id,
                                        "description": t.description,
                                        "elapsed_secs": t.elapsed_secs,
                                        "result": serde_json::to_value(&t.result).unwrap_or(json!(t.result.summary)),
                                    })
                                }).collect();
                                let result = json!({
                                    "active": active,
                                    "completed": completed_json,
                                })
                                .to_string();
                                outcomes.push(ToolOutcome::Result {
                                    index: ptc.index,
                                    id: ptc.id,
                                    name: ptc.name,
                                    args: ptc.args,
                                    result,
                                    elapsed_ms: t0.elapsed().as_millis(),
                                });
                            }
                            Some(BuiltinTool::SpawnAgent) => {
                                let run_in_background =
                                    ptc.args["run_in_background"].as_bool().unwrap_or(false);
                                let task_str = ptc.args["task"].as_str().unwrap_or("").to_string();

                                if run_in_background {
                                    let task_id =
                                        format!("bg_{}", &Uuid::new_v4().to_string()[..8]);
                                    let url = gateway_url.clone();
                                    let key = gateway_key.clone();
                                    let spawn_cfg =
                                        spawn::SpawnConfig::from_args(&ptc.args, task_str.clone());

                                    match self.background.spawn_task(
                                        task_id.clone(),
                                        task_str.clone(),
                                        async move {
                                            match spawn::spawn_agent(spawn_cfg, &url, &key).await {
                                                Ok(r) => r,
                                                Err(e) => spawn::AgentResult {
                                                    status: "error".to_string(),
                                                    summary: e.to_string(),
                                                    cost: 0.0,
                                                    turns: 0,
                                                },
                                            }
                                        },
                                    ) {
                                        Ok(()) => {
                                            self.permission_gate
                                                .renderer()
                                                .background_task_spawned(&task_id, &task_str);
                                            outcomes.push(ToolOutcome::Result {
                                                index: ptc.index,
                                                id: ptc.id,
                                                name: ptc.name,
                                                args: ptc.args,
                                                result: json!({
                                                    "status": "spawned_in_background",
                                                    "task_id": task_id,
                                                    "description": task_str,
                                                    "message": "Task is running in the background. You will be notified when it completes. Use check_background_tasks to query status."
                                                }).to_string(),
                                                elapsed_ms: 0,
                                            });
                                        }
                                        Err(msg) => {
                                            outcomes.push(ToolOutcome::Result {
                                                index: ptc.index,
                                                id: ptc.id,
                                                name: ptc.name,
                                                args: ptc.args,
                                                result: json!({"status": "error", "summary": msg})
                                                    .to_string(),
                                                elapsed_ms: 0,
                                            });
                                        }
                                    }
                                } else {
                                    let args = ptc.args;
                                    let index = ptc.index;
                                    let id = ptc.id;
                                    let name = ptc.name;
                                    let url = gateway_url.clone();
                                    let key = gateway_key.clone();
                                    let spawn_cfg = spawn::SpawnConfig::from_args(&args, task_str);
                                    joinset_pending.push((index, id.clone()));
                                    joinset.spawn(async move {
                                        let t0 = std::time::Instant::now();
                                        let result =
                                            match spawn::spawn_agent(spawn_cfg, &url, &key).await {
                                                Ok(r) => serde_json::to_string(&r)
                                                    .unwrap_or_else(|_| r.summary.clone()),
                                                Err(e) => serde_json::json!({
                                                    "status": "error",
                                                    "summary": e.to_string()
                                                })
                                                .to_string(),
                                            };
                                        ToolOutcome::Result {
                                            index,
                                            id,
                                            name,
                                            args,
                                            result,
                                            elapsed_ms: t0.elapsed().as_millis(),
                                        }
                                    });
                                }
                            }
                            _ => {
                                let name = ptc.name;
                                let args = ptc.args;
                                let index = ptc.index;
                                let id = ptc.id;
                                let mcp = mcp_arc.clone();
                                let cfg = task_config.clone();
                                let cwd = run_cwd.clone();
                                joinset_pending.push((index, id.clone()));
                                joinset.spawn(async move {
                                    let t0 = std::time::Instant::now();
                                    let tool_result = tools::dispatch(
                                        &name,
                                        &args,
                                        &cfg,
                                        Some(&cwd),
                                        mcp.as_deref(),
                                    )
                                    .await;
                                    let result = match tool_result {
                                        tools::ToolResult::Text(s) => s,
                                        tools::ToolResult::Multimodal(v) => v.to_string(),
                                    };
                                    ToolOutcome::Result {
                                        index,
                                        id,
                                        name,
                                        args,
                                        result,
                                        elapsed_ms: t0.elapsed().as_millis(),
                                    }
                                });
                            }
                        }
                    }

                    // Collect all parallel results
                    while let Some(join_result) = joinset.join_next().await {
                        match join_result {
                            Ok(outcome) => {
                                // Remove from pending so we know this ID was accounted for.
                                let idx = outcome.index();
                                joinset_pending.retain(|(i, _)| *i != idx);
                                outcomes.push(outcome);
                            }
                            Err(e) => {
                                tracing::warn!("tool task panicked: {e}");
                            }
                        }
                    }
                    // Any IDs still in joinset_pending belong to tasks that panicked.
                    // Push error outcomes so the model receives a tool response for each.
                    for (index, id) in joinset_pending {
                        outcomes.push(ToolOutcome::Denied {
                            index,
                            id,
                            message: "tool task panicked; result unavailable".to_string(),
                        });
                    }

                    // --- Phase 3: Sequential assembly — sort, post-hooks, push messages ---
                    outcomes.sort_by_key(|o| o.index());

                    // Classify turn before consuming outcomes (needed for exploration budget)
                    // A denied call means the model attempted an action (we can't inspect what kind),
                    // so treat it as non-read-only to avoid inflating the exploration streak.
                    let turn_all_readonly = outcomes.iter().all(|o| match o {
                        ToolOutcome::Result { name, .. } => is_read_only(name),
                        ToolOutcome::Denied { .. } => false,
                    });

                    let mut turn_files: Vec<String> = Vec::new();

                    for outcome in outcomes {
                        match outcome {
                            ToolOutcome::Denied { id, message, .. } => {
                                self.session.push_message(Message {
                                    role: MessageRole::Tool,
                                    content: Some(json!(message)),
                                    name: None,
                                    tool_calls: None,
                                    tool_call_id: Some(id),
                                    extra: Default::default(),
                                });
                            }
                            ToolOutcome::Result {
                                id,
                                name,
                                args,
                                result,
                                elapsed_ms,
                                ..
                            } => {
                                let result = truncate_tool_output(&name, &result, max_tool_output);
                                let result =
                                    self.hook_runner.run_post_hooks(&name, &args, &result).await;
                                let result_preview =
                                    common::truncate_with_ellipsis(result.trim_start(), 80);
                                self.permission_gate.renderer().tool_result(
                                    &name,
                                    elapsed_ms,
                                    result.len(),
                                    &result_preview,
                                );
                                self.session.push_message(Message {
                                    role: MessageRole::Tool,
                                    content: Some(json!(result)),
                                    name: None,
                                    tool_calls: None,
                                    tool_call_id: Some(id),
                                    extra: Default::default(),
                                });

                                // Track files mutated by write/edit tools
                                if matches!(
                                    BuiltinTool::from_str(&name),
                                    Some(BuiltinTool::WriteFile | BuiltinTool::EditFile)
                                ) && let Some(path) = args["path"].as_str()
                                {
                                    let normed = normalize_path(path);
                                    self.files_touched.insert(normed.clone());
                                    turn_files.push(normed);
                                }

                                // Count read-only calls toward the decision checkpoint
                                if is_read_only(&name) {
                                    self.exploration_count += 1;
                                }
                            }
                        }
                    }

                    // Inject skill content as user messages after tool results
                    for skill_content in pending_skill_messages {
                        self.session
                            .push_message(common::user_message(skill_content));
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
                            self.permission_gate.renderer().compile_check(&msg);
                            self.session.push_message(common::user_message(msg));
                        }
                    }

                    // Exploration budget: nudge after too many consecutive read-only turns
                    let had_substantive_text = content_buf.trim().len() > 100;
                    if let Some(nudge) =
                        exploration_budget.record_turn(turn_all_readonly, had_substantive_text)
                    {
                        tracing::info!("exploration budget nudge triggered");
                        self.permission_gate.renderer().exploration_nudge();
                        self.session.push_message(common::user_message(nudge));
                    }
                }
            }
        }

        // Flush any pending memory entries
        if let Err(e) = self.memory.flush().await {
            tracing::warn!("memory flush failed: {e}");
        }

        self.permission_gate.renderer().session_summary(
            &model_name,
            turns,
            total_prompt,
            total_completion,
            total_cost_usd,
            &self.session.episode_id.to_string()[..8],
        );
        if stop_reason == Some(AgentStopReason::Interrupt) {
            eprintln!(
                "Resume with: prism run --resume {}",
                &self.session.episode_id.to_string()[..8]
            );
        }

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

        self.emit_implicit_feedback(stop_reason, turns, total_cost_usd)
            .await;

        // Collect once for both file persistence and memory extraction
        let files_vec: Vec<String> = self.files_touched.iter().cloned().collect();

        // Persist files_touched for checkout hook consumption
        if !files_vec.is_empty() {
            let files_path = self
                .config
                .session
                .sessions_dir
                .join(format!("{}.files", self.session.episode_id));
            let content = files_vec.join("\n");
            if let Err(e) = std::fs::write(&files_path, &content) {
                tracing::warn!("failed to write files_touched: {e}");
            }
        }

        // Auto-extract memories from session signals (resolution patterns)
        if let Some((store, ws_id, agent_name)) = self.uh_context() {
            let thread_name = self.current_thread.as_deref();

            let mut session_findings: Vec<String> = Vec::new();
            for msg in self.session.active_messages() {
                if msg.role == prism_types::MessageRole::Assistant
                    && let Some(content) = msg.content.as_ref().and_then(|c| c.as_str())
                {
                    if content.len() < 500 {
                        let lower = content.to_lowercase();
                        if crate::memory::RESOLUTION_KEYWORDS
                            .iter()
                            .any(|kw| lower.contains(kw))
                        {
                            session_findings.push(content.to_string());
                        }
                    }
                }
            }

            if !session_findings.is_empty() {
                let extracted = crate::memory::auto_extract_memories(
                    store,
                    ws_id,
                    &agent_name,
                    &[],
                    &files_vec,
                    &session_findings,
                    thread_name,
                )
                .await;
                if !extracted.is_empty() {
                    tracing::debug!(
                        count = extracted.len(),
                        "auto-extracted memories from session"
                    );
                }
            }
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

    pub fn files_touched(&self) -> Vec<String> {
        self.files_touched.iter().cloned().collect()
    }

    /// Returns (store, workspace_id, agent_name) if uglyhat is configured.
    fn uh_context(&self) -> Option<(&dyn uglyhat::store::Store, uuid::Uuid, String)> {
        let store = self.memory.store()?;
        let ws_id = self.memory.workspace_id()?;
        let agent_name =
            std::env::var(UH_AGENT_NAME_ENV).unwrap_or_else(|_| UH_AGENT_NAME_DEFAULT.to_string());
        Some((store.as_ref(), ws_id, agent_name))
    }

    /// Send heartbeat and set agent state to Working via uglyhat store.
    async fn send_heartbeat_and_set_working(&self) {
        use uglyhat::model::AgentState;

        let Some((store, ws_id, agent_name)) = self.uh_context() else {
            return;
        };

        let _ = store.heartbeat(ws_id, &agent_name).await;
        let _ = store
            .set_agent_state(ws_id, &agent_name, AgentState::Working)
            .await;
    }

    /// Poll for pending decision notifications and inject them as a system message.
    async fn poll_and_inject_decisions(&mut self) {
        use uglyhat::store::Store;

        // Need an owned store clone because we later mutably borrow self.session
        let Some(store) = self.memory.store().cloned() else {
            return;
        };
        let Some(ws_id) = self.memory.workspace_id() else {
            return;
        };
        let agent_name =
            std::env::var(UH_AGENT_NAME_ENV).unwrap_or_else(|_| UH_AGENT_NAME_DEFAULT.to_string());

        let decisions = match store
            .pending_decision_notifications(ws_id, &agent_name)
            .await
        {
            Ok(d) if !d.is_empty() => d,
            _ => return,
        };

        let mut msg =
            String::from("NEW DECISIONS FROM OTHER AGENTS (acknowledge and incorporate):\n");
        let mut ids = Vec::new();
        for d in &decisions {
            msg.push_str(&format!("- [{}] {}: {}\n", d.scope, d.title, d.content));
            ids.push(d.id);
        }

        self.session.push_message(common::user_message(msg));

        // Auto-acknowledge
        let _ = store.acknowledge_decisions(ws_id, &agent_name, ids).await;
    }

    /// Emit an implicit quality feedback event based on session outcome signals.
    async fn emit_implicit_feedback(
        &self,
        stop_reason: Option<AgentStopReason>,
        turns: u32,
        cost_usd: f64,
    ) {
        let has_branches = self.session.tree.as_ref().is_some_and(|t| t.has_branches());
        let quality =
            compute_implicit_quality(stop_reason, has_branches, self.plan_mode_violations);

        let metadata = serde_json::json!({
            "source": "implicit",
            "stop_reason": stop_reason.map(|r| r.to_session_str()),
            "turns": turns,
            "cost_usd": cost_usd,
            "model": self.session.model,
            "plan_mode_violations": self.plan_mode_violations,
        });

        let _ = self
            .client
            .post_feedback(
                None,
                Some(self.session.episode_id),
                "implicit_quality",
                quality,
                metadata,
            )
            .await;
    }

    /// Set up uglyhat guardrails for structural plan mode enforcement.
    /// Creates a thread (or reuses the current one) and restricts:
    /// - `allowed_files` to the plan file only
    /// - `allowed_tools` to everything except bash/run_command (shell escape vectors)
    ///
    /// Returns true if structural enforcement was activated.
    async fn setup_plan_mode_guardrails(&mut self) -> bool {
        use uglyhat::store::Store;

        let plan_file = match self.plan_file.clone() {
            Some(pf) => pf,
            None => return false,
        };

        let store = match self.memory.store().cloned() {
            Some(s) => s,
            None => {
                tracing::warn!(
                    "plan mode: no uglyhat context — falling back to prompt-for-everything"
                );
                return false;
            }
        };
        let ws_id = match self.memory.workspace_id() {
            Some(id) => id,
            None => {
                tracing::warn!("plan mode: no workspace — falling back to prompt-for-everything");
                return false;
            }
        };

        // Resolve or auto-create the thread
        let thread_name = if let Some(ref t) = self.current_thread {
            t.clone()
        } else {
            let id = format!("plan-{}", &Uuid::new_v4().to_string()[..8]);
            match store
                .create_thread(ws_id, &id, "auto-created for structural plan mode", vec![])
                .await
            {
                Ok(_) => {
                    self.current_thread = Some(id.clone());
                    self.plan_thread_auto_created = true;
                    id
                }
                Err(e) => {
                    tracing::warn!("plan mode: failed to create thread: {e}");
                    return false;
                }
            }
        };

        let allowed_tools = BuiltinTool::all_non_shell()
            .iter()
            .map(|t| t.as_str().to_string())
            .collect();
        let guardrails = make_guardrails(ws_id, vec![plan_file], allowed_tools);

        match store.set_guardrails(ws_id, &thread_name, guardrails).await {
            Ok(_) => {
                tracing::info!(thread = %thread_name, "plan mode: structural guardrails active");
                true
            }
            Err(e) => {
                tracing::warn!("plan mode: failed to set guardrails: {e}");
                false
            }
        }
    }

    /// Clean up plan mode guardrails at session end.
    async fn teardown_plan_mode_guardrails(&self) {
        use uglyhat::store::Store;

        let thread_name = match self.current_thread.as_deref() {
            Some(t) => t,
            None => return,
        };
        let store = match self.memory.store().cloned() {
            Some(s) => s,
            None => return,
        };
        let ws_id = match self.memory.workspace_id() {
            Some(id) => id,
            None => return,
        };

        if self.plan_thread_auto_created {
            let _ = store.archive_thread(ws_id, thread_name).await;
        } else {
            // Clear guardrails on the pre-existing thread
            let _ = store
                .set_guardrails(ws_id, thread_name, make_guardrails(ws_id, vec![], vec![]))
                .await;
        }
    }

    /// Check thread-scoped guardrails. Returns Some(denial_message) if denied.
    async fn check_guardrails(&self, tool_name: &str, args: &serde_json::Value) -> Option<String> {
        let thread_name = self.current_thread.as_deref()?;
        let (store, ws_id, agent_name) = self.uh_context()?;

        // Only pass file_path for write tools — allowed_files semantically means "files you
        // may mutate." Passing it for reads would block legitimate reads on restricted paths.
        let file_path = guardrail_file_path(tool_name, args);

        match store
            .check_guardrail(ws_id, thread_name, &agent_name, tool_name, file_path)
            .await
        {
            Ok(check) if !check.allowed => Some(format!(
                "Guardrail violation: {}",
                check.reason.unwrap_or_else(|| "access denied".to_string())
            )),
            _ => None,
        }
    }

    /// Set agent state to Idle (called from REPL at prompt).
    pub async fn set_idle(&self) {
        use uglyhat::model::AgentState;

        let Some((store, ws_id, agent_name)) = self.uh_context() else {
            return;
        };

        let _ = store
            .set_agent_state(ws_id, &agent_name, AgentState::Idle)
            .await;
    }

    /// Handle the `record_decision` tool — persists a decision with rationale to uglyhat.
    async fn handle_record_decision(&self, args: &serde_json::Value) -> String {
        use uglyhat::model::DecisionScope;

        let Some((store, ws_id, _agent_name)) = self.uh_context() else {
            return json!({"error": "no uglyhat context store available"}).to_string();
        };

        let title = args["title"].as_str().unwrap_or("").to_string();
        let content = args["content"].as_str().unwrap_or("").to_string();
        if title.is_empty() {
            return json!({"error": "title is required"}).to_string();
        }

        // Resolve thread: explicit arg takes precedence, then current_thread
        let thread_name = args["thread"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or(self.current_thread.as_deref());

        let thread_id = if let Some(name) = thread_name {
            match store.get_thread(ws_id, name).await {
                Ok(t) => Some(t.id),
                Err(_) => None,
            }
        } else {
            None
        };

        let tags = common::parse_str_array(&args["tags"]);

        let scope = match args["scope"].as_str() {
            Some("workspace") => DecisionScope::Workspace,
            _ => DecisionScope::Thread,
        };

        match store
            .save_decision(ws_id, &title, &content, thread_id, tags, scope.clone())
            .await
        {
            Ok(d) => json!({
                "recorded": true,
                "id": d.id.to_string(),
                "title": d.title,
                "scope": scope.to_string(),
            })
            .to_string(),
            Err(e) => json!({"error": e.to_string()}).to_string(),
        }
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
            let tags = common::parse_str_array(&args["tags"]);

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

fn make_guardrails(
    ws_id: uuid::Uuid,
    allowed_files: Vec<String>,
    allowed_tools: Vec<String>,
) -> uglyhat::model::ThreadGuardrails {
    uglyhat::model::ThreadGuardrails {
        id: uuid::Uuid::new_v4(),
        thread_id: uuid::Uuid::nil(),
        workspace_id: ws_id,
        owner_agent_id: None,
        locked: false,
        allowed_files,
        allowed_tools,
        cost_budget_usd: None,
        cost_spent_usd: 0.0,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn compute_implicit_quality(
    stop_reason: Option<AgentStopReason>,
    has_branches: bool,
    plan_mode_violations: u32,
) -> f64 {
    let mut quality: f64 = 0.70;
    match stop_reason {
        Some(AgentStopReason::Stop) => quality += 0.15,
        Some(AgentStopReason::Interrupt) => quality -= 0.30,
        Some(AgentStopReason::CostCap) => quality -= 0.10,
        Some(AgentStopReason::MaxTurns) => quality -= 0.20,
        None => quality -= 0.20,
    }
    if has_branches {
        quality -= 0.15;
    }
    if plan_mode_violations > 0 {
        quality -= 0.40;
    }
    quality.clamp(0.0, 1.0)
}

/// Extract the file path from tool args for guardrail checking.
/// Only write tools carry a meaningful path restriction — reads are intentionally excluded
/// so `allowed_files` doesn't block reading files outside the plan file.
fn guardrail_file_path<'a>(tool_name: &str, args: &'a serde_json::Value) -> Option<&'a str> {
    match BuiltinTool::from_str(tool_name) {
        Some(BuiltinTool::WriteFile | BuiltinTool::EditFile) => args["path"].as_str(),
        _ => None,
    }
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

    #[test]
    fn implicit_quality_baseline() {
        let q = compute_implicit_quality(Some(AgentStopReason::Stop), false, 0);
        assert!((q - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn implicit_quality_plan_mode_violation_penalty() {
        let without = compute_implicit_quality(Some(AgentStopReason::Stop), false, 0);
        let with = compute_implicit_quality(Some(AgentStopReason::Stop), false, 1);
        assert!((without - with - 0.40).abs() < f64::EPSILON);
    }

    #[test]
    fn implicit_quality_plan_violations_clamp_to_zero() {
        // Interrupt (-0.30) + branches (-0.15) + plan violations (-0.40) = 0.70 - 0.85 < 0
        let q = compute_implicit_quality(Some(AgentStopReason::Interrupt), true, 3);
        assert!((q - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn implicit_quality_multiple_violations_same_as_one() {
        // Penalty is flat -0.40 regardless of count
        let one = compute_implicit_quality(Some(AgentStopReason::Stop), false, 1);
        let many = compute_implicit_quality(Some(AgentStopReason::Stop), false, 5);
        assert!((one - many).abs() < f64::EPSILON);
    }

    #[test]
    fn check_guardrails_only_passes_file_path_for_write_tools() {
        use serde_json::json;
        let args = json!({"path": "/sensitive/file.rs", "command": "cat /etc/passwd"});

        // Write tools carry the path; reads and shell tools pass None
        assert_eq!(
            guardrail_file_path("write_file", &args),
            Some("/sensitive/file.rs")
        );
        assert_eq!(
            guardrail_file_path("edit_file", &args),
            Some("/sensitive/file.rs")
        );
        for tool in [
            "read_file",
            "bash",
            "run_command",
            "glob_files",
            "grep_files",
        ] {
            assert_eq!(
                guardrail_file_path(tool, &args),
                None,
                "'{tool}' must not extract file_path"
            );
        }
    }

    #[test]
    fn normalize_path_strips_cwd_prefix() {
        let cwd = std::env::current_dir().unwrap();
        let abs = cwd.join("src/main.rs").to_string_lossy().into_owned();
        assert_eq!(normalize_path(&abs), "src/main.rs");
    }

    #[test]
    fn normalize_path_keeps_outside_paths() {
        assert_eq!(normalize_path("/tmp/other/file.rs"), "/tmp/other/file.rs");
    }

    #[test]
    fn exploration_budget_fires_at_threshold() {
        let mut budget = ExplorationBudget::new(3);
        assert!(budget.record_turn(true, false).is_none());
        assert!(budget.record_turn(true, false).is_none());
        let nudge = budget.record_turn(true, false);
        assert!(nudge.is_some());
        assert!(nudge.unwrap().contains("3 consecutive"));
    }

    #[test]
    fn exploration_budget_fires_only_once_per_streak() {
        let mut budget = ExplorationBudget::new(2);
        budget.record_turn(true, false);
        let first = budget.record_turn(true, false);
        assert!(first.is_some());
        // Fourth read-only turn — already nudged, should not fire again
        let second = budget.record_turn(true, false);
        assert!(second.is_none());
    }

    #[test]
    fn exploration_budget_resets_on_write_turn() {
        let mut budget = ExplorationBudget::new(2);
        budget.record_turn(true, false);
        budget.record_turn(true, false); // fires nudge
        // Write turn resets the streak
        budget.record_turn(false, false);
        assert!(budget.record_turn(true, false).is_none());
        // Fires again after a full new streak
        let nudge = budget.record_turn(true, false);
        assert!(nudge.is_some());
    }

    #[test]
    fn exploration_budget_disabled_when_threshold_zero() {
        let mut budget = ExplorationBudget::new(0);
        for _ in 0..20 {
            assert!(budget.record_turn(true, false).is_none());
        }
    }

    #[test]
    fn exploration_budget_resets_on_substantive_text() {
        let mut budget = ExplorationBudget::new(2);
        budget.record_turn(true, false);
        // Substantive text output counts as a non-exploration turn
        budget.record_turn(true, true);
        assert!(budget.record_turn(true, false).is_none());
    }

    #[test]
    fn decision_signal_detection() {
        assert!(has_decision_signals(
            "Please decide between JWT and sessions"
        ));
        assert!(has_decision_signals(
            "Compare Option A vs Option B before implementation"
        ));
        assert!(has_decision_signals("What are the trade-offs?"));
        assert!(has_decision_signals("choose between axum and actix"));
        assert!(has_decision_signals("pros and cons of each approach"));
        assert!(!has_decision_signals("Implement the login endpoint"));
        assert!(!has_decision_signals("Fix the bug in auth.rs"));
    }
}
