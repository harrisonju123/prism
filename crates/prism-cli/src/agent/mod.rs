pub mod background;
pub mod spawn;

use anyhow::{Result, anyhow};
use chrono::Utc;
use futures::StreamExt;
use prism_client::PrismClient;
use prism_types::{ChatCompletionRequest, Message, MessageRole};
use regex::Regex;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
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
use crate::tools::cache::ToolResultCache;

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

/// Extract "named anchors" from file content: exports, routes, nav links, and top-level Rust fns.
/// Used by Guard C to detect when a write would silently remove existing named items.
fn extract_named_anchors(content: &str) -> HashSet<String> {
    static EXPORT_RE: OnceLock<Regex> = OnceLock::new();
    static ROUTE_RE: OnceLock<Regex> = OnceLock::new();
    static NAV_RE: OnceLock<Regex> = OnceLock::new();
    static FN_RE: OnceLock<Regex> = OnceLock::new();

    let export_re = EXPORT_RE.get_or_init(|| {
        Regex::new(r#"export\s+(?:function|class|const|type|interface)\s+(\w+)"#).unwrap()
    });
    let route_re = ROUTE_RE.get_or_init(|| Regex::new(r#"path=["']([^"']+)["']"#).unwrap());
    let nav_re = NAV_RE.get_or_init(|| Regex::new(r#"\bto=["']([^"']+)["']"#).unwrap());
    let fn_re =
        FN_RE.get_or_init(|| Regex::new(r#"(?m)^(?:pub\s+)?(?:async\s+)?fn\s+(\w+)"#).unwrap());

    let mut anchors = HashSet::new();
    for cap in export_re.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            anchors.insert(m.as_str().to_string());
        }
    }
    for cap in route_re.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            anchors.insert(format!("route:{}", m.as_str()));
        }
    }
    for cap in nav_re.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            anchors.insert(format!("nav:{}", m.as_str()));
        }
    }
    for cap in fn_re.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            anchors.insert(format!("fn:{}", m.as_str()));
        }
    }
    anchors
}

/// Classify TypeScript/compiler error output to detect environment-level issues
/// (missing @types, tsconfig misconfiguration) vs real code bugs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ErrorClass {
    /// Missing @types, jsx-runtime, tsconfig — cannot be fixed by editing source files.
    Environment,
    /// Type mismatches, undefined vars, wrong props — fixable via code edits.
    Code,
    Unknown,
}

fn classify_ts_errors(output: &str) -> ErrorClass {
    const ENV_PATTERNS: &[&str] = &[
        "Cannot find module",
        "could not find declaration file",
        "JSX element implicitly has type 'any' because no interface 'JSX.IntrinsicElements'",
        "requires the module path 'react/jsx-runtime' to exist",
        "Cannot find name 'React'",
    ];
    const CODE_PATTERNS: &[&str] = &[
        "is not assignable to type",
        "does not exist on type",
        "Property '",
        "Argument of type",
        "Type '",
    ];

    let is_env = ENV_PATTERNS.iter().any(|p| output.contains(p));
    let is_code = CODE_PATTERNS.iter().any(|p| output.contains(p));

    match (is_env, is_code) {
        (true, false) => ErrorClass::Environment,
        (_, true) => ErrorClass::Code,
        _ => ErrorClass::Unknown,
    }
}

/// Returns true if the assistant text contains common completion signals.
fn has_completion_signals(text: &str) -> bool {
    let lower = text.to_lowercase();
    const SIGNALS: &[&str] = &[
        "done",
        "implemented",
        "added",
        "wired",
        "here's what",
        "here is what",
        "complete",
        "finished",
        "all set",
    ];
    SIGNALS.iter().any(|s| lower.contains(s))
}

const DECISION_CHECKPOINT_THRESHOLD_DEFAULT: u32 = 8;
const QUESTION_CHECKPOINT_INTERVAL_DEFAULT: u32 = 5;
const FILE_CLAIM_TTL_SECS: i64 = 3600; // 1 hour

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

/// Fires a nudge when the same file is written twice within `window` turns.
struct WriteCoalesceTracker {
    /// path → turn number of last write
    last_write_turn: HashMap<String, u32>,
    window: u32,
    /// Paths already nudged in the current window to avoid repeat spam.
    nudged: HashSet<String>,
}

impl WriteCoalesceTracker {
    fn new(window: u32) -> Self {
        Self {
            last_write_turn: HashMap::new(),
            window,
            nudged: HashSet::new(),
        }
    }

    /// Record a write. Returns `Some(nudge)` if this is a repeat within the window.
    fn record_write(&mut self, path: &str, current_turn: u32) -> Option<String> {
        if self.window == 0 {
            return None; // disabled
        }
        let prev = self.last_write_turn.insert(path.to_string(), current_turn);
        if let Some(prev_turn) = prev {
            if current_turn - prev_turn < self.window && !self.nudged.contains(path) {
                self.nudged.insert(path.to_string());
                return Some(format!(
                    "[System] You've rewritten `{path}` twice within {} turns. \
                     Consider using edit_file to make targeted changes instead of full rewrites.",
                    self.window
                ));
            }
        }
        // Reset nudge suppression when the file moves outside the window
        if prev.map_or(true, |p| current_turn - p >= self.window) {
            self.nudged.remove(path);
        }
        None
    }
}

/// Fires a nudge when the agent produces N consecutive long-text turns without tool calls,
/// indicating it is restating context rather than executing.
struct VerbosityTracker {
    /// Consecutive turns where assistant text exceeded `char_threshold` with no tool calls.
    consecutive_verbose_turns: u32,
    /// Character count threshold for "verbose" (default ~2000 chars ≈ ~500 tokens).
    char_threshold: usize,
    /// How many consecutive verbose turns before nudge fires.
    turn_threshold: u32,
    /// True after nudge fired for the current streak; clears when streak resets.
    streak_nudged: bool,
}

impl VerbosityTracker {
    fn new(char_threshold: usize, turn_threshold: u32) -> Self {
        Self {
            consecutive_verbose_turns: 0,
            char_threshold,
            turn_threshold,
            streak_nudged: false,
        }
    }

    /// Record a turn. Returns `Some(nudge)` when threshold is hit for the first time in a streak.
    /// Increments streak when text exceeds the threshold (regardless of tool calls).
    /// Resets streak when the turn had tool calls with short/no text (pure execution mode).
    fn record_turn(&mut self, text_len: usize, had_tool_calls: bool) -> Option<String> {
        if self.turn_threshold == 0 {
            return None; // disabled
        }

        if text_len > self.char_threshold {
            // Long text output — verbose turn regardless of tool calls
            self.consecutive_verbose_turns += 1;
        } else if had_tool_calls {
            // Tool calls with terse text — execution mode, reset streak
            self.consecutive_verbose_turns = 0;
            self.streak_nudged = false;
        }

        if self.consecutive_verbose_turns >= self.turn_threshold && !self.streak_nudged {
            self.streak_nudged = true;
            Some(format!(
                "[Verbosity Check] Your last {} responses were long text blocks.\n\
                 You are likely restating context the user already has. Instead:\n\
                 - State only what CHANGED since your last response\n\
                 - If planning, write the plan to a file — don't repeat it in chat\n\
                 - If ready to implement, start using tools instead of describing what you'll do",
                self.turn_threshold
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

enum ReviewOutcome {
    Approved,
    ChangesRequested(String),
    Rejected,
    Interrupted,
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
    pub config: Config,
    pub session: Session,
    memory: MemoryManager,
    pub mcp_registry: Option<Arc<McpRegistry>>,
    pub skill_registry: SkillRegistry,
    permission_gate: ToolPermissionGate,
    hook_runner: HookRunner,
    compressor: Option<ContextCompressor>,
    background: BackgroundTaskManager,
    // Shared interrupt flag — set by ctrl-c handler (owned by REPL) or per-run handler
    pub interrupted: Arc<AtomicBool>,
    // Counts ctrl-c presses within a single run; shared with the spawned signal task.
    // Kept as a field so two-press kill logic persists correctly across REPL turns.
    interrupt_count: Arc<AtomicU32>,
    /// Current thread name (from PRISM_THREAD env or handoff)
    pub current_thread: Option<String>,
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
    pub additional_dirs: Vec<std::path::PathBuf>,
    /// True when the user's prompt contains decision-required signals.
    decision_checkpoint_armed: bool,
    /// Cumulative count of read-only exploration tool calls in current run.
    exploration_count: u32,
    /// True after the decision checkpoint nudge has fired (one-shot per run).
    decision_checkpoint_fired: bool,
    /// Turn number at which the last question checkpoint fired (0 = never).
    last_question_checkpoint_turn: u32,
    /// Files written via write_file (full overwrite) this session — for repeat-rewrite detection.
    files_full_written: HashSet<String>,
    /// New file paths already prompted for design confirmation — prevents the gate from re-firing.
    new_file_design_prompted: HashSet<String>,
    /// True once plan mode has been auto-activated due to 3+ new files in a session.
    new_feature_plan_triggered: bool,
    /// mtime of files when last read this session — for staleness detection.
    files_read_mtime: HashMap<String, std::time::SystemTime>,
    /// Paths already warned by Guard C — second attempt on same path is allowed (acknowledged).
    guard_c_warned: HashSet<String>,
    /// When Some, inject a self-review prompt at the start of the next turn.
    pending_self_review: Option<Vec<String>>,
    /// Model pricing cache: model name → (input_per_1m_usd, output_per_1m_usd).
    pricing_cache: HashMap<String, (f64, f64)>,
    /// True after first populate attempt (success or failure) — prevents retry loops.
    pricing_cache_populated: bool,
    /// files_touched.len() at last session checkpoint — for change-detection.
    last_checkpoint_file_count: usize,
    /// Guardrail cost accumulated since last 5-turn flush (avoids per-turn DB writes).
    accumulated_guardrail_cost: f64,
    /// Constraints from the active handoff — enforced in the tool dispatch loop.
    handoff_constraints: Option<prism_context::model::HandoffConstraints>,
    /// Session-scoped cache for read-only tool results (read_file, list_dir, glob_files, grep_files).
    tool_cache: Arc<ToolResultCache>,
}

const PRISM_THREAD_ENV: &str = "PRISM_THREAD";

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
        let current_thread = std::env::var(PRISM_THREAD_ENV)
            .ok()
            .or_else(|| std::env::var("UH_THREAD").ok());
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
            files_full_written: HashSet::new(),
            new_file_design_prompted: HashSet::new(),
            files_read_mtime: HashMap::new(),
            new_feature_plan_triggered: false,
            guard_c_warned: HashSet::new(),
            pending_self_review: None,
            pricing_cache: HashMap::new(),
            pricing_cache_populated: false,
            last_checkpoint_file_count: 0,
            accumulated_guardrail_cost: 0.0,
            handoff_constraints: None,
            tool_cache: ToolResultCache::new(),
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
        let current_thread = std::env::var(PRISM_THREAD_ENV)
            .ok()
            .or_else(|| std::env::var("UH_THREAD").ok());
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
            files_full_written: HashSet::new(),
            new_file_design_prompted: HashSet::new(),
            files_read_mtime: HashMap::new(),
            new_feature_plan_triggered: false,
            guard_c_warned: HashSet::new(),
            pending_self_review: None,
            pricing_cache: HashMap::new(),
            pricing_cache_populated: false,
            last_checkpoint_file_count: 0,
            accumulated_guardrail_cost: 0.0,
            handoff_constraints: None,
            tool_cache: ToolResultCache::new(),
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

        // If spawned as a child agent, accept the handoff so the parent can track progress.
        if let Ok(hid_str) = std::env::var("PRISM_HANDOFF_ID") {
            if let Ok(hid) = hid_str.parse::<uuid::Uuid>() {
                use prism_context::store::Store;
                let agent_name = crate::config::agent_name_from_env();
                if let (Some(store), Some(ws_id)) =
                    (self.memory.store().cloned(), self.memory.workspace_id())
                {
                    if let Ok(accepted) = store.accept_handoff(ws_id, hid, &agent_name).await {
                        self.handoff_constraints = Some(accepted.constraints);
                        // Transition Accepted → Running immediately
                        let _ = store.start_handoff(ws_id, hid).await;
                    }
                }
            }
        }

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
        loop {
            let result = self.inner_run_impl().await;
            if self.plan_file.is_some() {
                self.teardown_plan_mode_guardrails().await;
            }

            if !self.config.session.await_review {
                return result;
            }

            // Only gate on clean Stop
            let is_clean_stop = self.session.stop_reason.as_deref()
                == Some(AgentStopReason::Stop.to_session_str());
            if !is_clean_stop {
                return result;
            }

            let entry_id = match self.create_review_approval_entry().await {
                Some(id) => id,
                None => return result,
            };
            self.set_agent_state(prism_context::model::AgentState::AwaitingReview).await;
            eprintln!("[await-review] Session complete. Waiting for human review... (Ctrl+C to abort)");

            match self.poll_for_review_resolution(entry_id).await {
                ReviewOutcome::Approved => return result,
                ReviewOutcome::ChangesRequested(msg) => {
                    eprintln!("[await-review] Changes requested — re-entering agent loop");
                    self.session.push_message(crate::common::user_message(format!(
                        "REVIEW FEEDBACK (changes requested):\n{msg}\n\nPlease address the feedback above."
                    )));
                    self.session.stop_reason = None;
                    continue;
                }
                ReviewOutcome::Rejected => {
                    self.set_agent_state(prism_context::model::AgentState::Idle).await;
                    anyhow::bail!("review rejected by reviewer");
                }
                ReviewOutcome::Interrupted => return result,
            }
        }
    }

    async fn inner_run_impl(&mut self) -> Result<()> {
        // Populate pricing cache from gateway on first run (one attempt; fall back to hardcoded).
        if !self.pricing_cache_populated {
            self.populate_pricing_cache().await;
        }

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
        let write_coalesce_window = std::env::var("PRISM_WRITE_COALESCE_WINDOW")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3u32);
        let mut write_coalesce = WriteCoalesceTracker::new(write_coalesce_window);
        let mut verbosity_tracker = VerbosityTracker::new(
            std::env::var("PRISM_VERBOSITY_CHAR_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(2000),
            std::env::var("PRISM_VERBOSITY_TURN_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
        );
        let decision_threshold = decision_checkpoint_threshold();
        let q_interval = question_checkpoint_interval();
        // Exponential moving average of per-turn cost — used for CostSpike detection.
        let mut rolling_avg_turn_cost: f64 = 0.0;

        for _turn in 0..self.config.model.max_turns {
            if self.interrupted.load(Ordering::Relaxed) {
                self.permission_gate.renderer().interrupt_notice();
                stop_reason = Some(AgentStopReason::Interrupt);
                break;
            }

            // Heartbeat + state transition to Working
            self.send_heartbeat_and_set_working(turns).await;

            // Poll for workspace-scoped decisions from other agents
            self.poll_and_inject_decisions().await;

            // Poll inbox for messages sent to this agent from the IDE or other agents
            self.poll_and_inject_messages().await;

            // Poll supervisory inbox entries (approvals, cost spikes, risks from other agents)
            self.poll_and_inject_inbox().await;

            // --- Exploration checkpoints ---
            let current_turn = turns;

            // A) Decision checkpoint: fire once after enough exploration calls
            if self.decision_checkpoint_armed
                && !self.decision_checkpoint_fired
                && self.exploration_count >= decision_threshold
            {
                self.decision_checkpoint_fired = true;
                self.session.push_message(common::user_message(
                    "[Decision Checkpoint] You have explored enough context. \
The user asked you to decide between approaches before implementing. \
STOP exploring and present your findings now:\n\
1. List the options you've identified (Option A, Option B, etc.)\n\
2. For each option, state trade-offs (pros/cons)\n\
3. Give your recommendation with rationale\n\
4. Ask the user any clarifying questions that would affect the choice\n\
5. Use `record_decision` to persist your recommendation once confirmed\n\
6. Wait for user confirmation before implementing"
                        .to_string(),
                ));
                tracing::info!(
                    exploration_count = self.exploration_count,
                    "decision checkpoint fired"
                );
            }

            // B) Question checkpoint: every N turns, remind agent to surface unknowns
            if q_interval > 0
                && current_turn > 0
                && current_turn % q_interval == 0
                && current_turn != self.last_question_checkpoint_turn
            {
                self.last_question_checkpoint_turn = current_turn;
                self.session.push_message(common::user_message(
                    "[Question Checkpoint] You've been working for several turns. \
Before continuing, consider:\n\
- Are there ambiguities in the requirements you should ask about?\n\
- Are you making assumptions the user should confirm?\n\
- Is this heading in the direction the user expects?\n\
If you have questions, ask them now. If you're confident, continue."
                        .to_string(),
                ));
                tracing::info!(turn = current_turn, "question checkpoint fired");
            }

            // Self-review nudge: inject once after a turn where files were written and
            // the agent signalled completion. Fires at the start of the following turn.
            if let Some(files) = self.pending_self_review.take() {
                let file_list = files.join(", ");
                self.session.push_message(common::user_message(format!(
                    "[Self-review] Before declaring complete, verify:\n\
                     1. No existing exports, routes, or nav items were removed from: {file_list}\n\
                     2. The implementation matches the original request\n\
                     3. If you removed anything intentionally, state it explicitly."
                )));
                tracing::info!(files = %file_list, "self-review nudge injected");
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

            let mut stream = match self.client.stream_chat_completion(&req).await {
                Ok(s) => s,
                Err(e) => {
                    if e.is_budget_exceeded() {
                        let thread_ref_id = self.current_thread_id().await;
                        self.create_inbox_event(
                            prism_context::model::InboxEntryType::CostSpike,
                            "Gateway budget exceeded",
                            "The gateway rejected the request because the virtual key budget has been exhausted (HTTP 402).",
                            prism_context::model::InboxSeverity::Critical,
                            "thread",
                            thread_ref_id,
                        )
                        .await;
                    }
                    return Err(anyhow!("stream_chat_completion failed: {e}"));
                }
            };

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

                    let (in_rate, out_rate): (f64, f64) =
                        self.pricing_cache.get(model_name.as_str())
                            .copied()
                            .unwrap_or_else(|| fallback_cost_rate(&model_name));
                    turn_cost = (u.prompt_tokens as f64 * in_rate
                        + u.completion_tokens as f64 * out_rate)
                        / 1_000_000.0;
                    total_cost_usd += turn_cost;
                }
            }

            // Accumulate guardrail cost — flushed to DB every 5 turns to batch writes.
            if turn_cost > 0.0 {
                self.accumulated_guardrail_cost += turn_cost;
            }
            if turns > 0 && turns % 5 == 0 && self.accumulated_guardrail_cost > 0.0 {
                let thread = self.current_thread.clone();
                let cost = std::mem::replace(&mut self.accumulated_guardrail_cost, 0.0);
                if let Some(thread) = thread
                    && let Some((store, ws_id, _)) = self.context_store()
                {
                    let _ = store.increment_guardrail_cost(ws_id, &thread, cost).await;
                }
            }

            // Detect cost spikes — only after the first real turn (rolling_avg > 0)
            if turn_cost > 0.0 && rolling_avg_turn_cost > 0.0 {
                let is_spike = turn_cost > 1.5 * rolling_avg_turn_cost;
                let is_near_cap = self
                    .config
                    .model
                    .max_cost_usd
                    .is_some_and(|cap| total_cost_usd >= 0.8 * cap);
                if is_spike || is_near_cap {
                    let severity = if is_near_cap {
                        prism_context::model::InboxSeverity::Critical
                    } else {
                        prism_context::model::InboxSeverity::Warning
                    };
                    let title = if is_near_cap {
                        format!(
                            "Cost at {:.0}% of cap (${:.4} / ${:.4})",
                            100.0 * total_cost_usd / self.config.model.max_cost_usd.unwrap(),
                            total_cost_usd,
                            self.config.model.max_cost_usd.unwrap(),
                        )
                    } else {
                        format!(
                            "Turn cost spike: ${:.4} vs avg ${:.4}",
                            turn_cost, rolling_avg_turn_cost,
                        )
                    };
                    let thread_ref_id = self.current_thread_id().await;
                    self.create_inbox_event(
                        prism_context::model::InboxEntryType::CostSpike,
                        &title,
                        "",
                        severity,
                        "thread",
                        thread_ref_id,
                    )
                    .await;
                }
            }
            // Update rolling average (EMA with α=0.3) after spike check
            if turn_cost > 0.0 {
                rolling_avg_turn_cost = if rolling_avg_turn_cost == 0.0 {
                    turn_cost
                } else {
                    0.7 * rolling_avg_turn_cost + 0.3 * turn_cost
                };
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
                            // Surface denied write/execute tools as Approval entries so the
                            // dashboard operator can decide whether to unblock the agent.
                            if !is_read_only(&name) {
                                let thread_ref = self.current_thread_id().await;
                                self.create_inbox_event(
                                    prism_context::model::InboxEntryType::Approval,
                                    &format!("Tool denied: {name}"),
                                    &format!(
                                        "Agent requested `{name}` but was denied. \
                                         Review and approve if the action is safe.",
                                    ),
                                    prism_context::model::InboxSeverity::Warning,
                                    "thread",
                                    thread_ref,
                                )
                                .await;
                            }
                            outcomes.push(ToolOutcome::Denied {
                                index,
                                id,
                                message: permissions::PERMISSION_DENIED_MSG.to_string(),
                            });
                            continue;
                        }

                        // Handoff constraint enforcement
                        if let Some(ref constraints) = self.handoff_constraints {
                            if !constraints.allowed_tools.is_empty()
                                && !constraints.allowed_tools.iter().any(|t| t == &name)
                            {
                                outcomes.push(ToolOutcome::Denied {
                                    index,
                                    id,
                                    message: format!(
                                        "tool '{name}' not allowed by handoff constraints (allowed: {})",
                                        constraints.allowed_tools.join(", ")
                                    ),
                                });
                                continue;
                            }
                            let file_path_arg = args["path"]
                                .as_str()
                                .or_else(|| args["file_path"].as_str());
                            if let Some(fp) = file_path_arg {
                                if !constraints.allowed_files.is_empty()
                                    && !constraints.allowed_files.iter().any(|f| f == fp)
                                {
                                    outcomes.push(ToolOutcome::Denied {
                                        index,
                                        id,
                                        message: format!(
                                            "file '{fp}' not allowed by handoff constraints"
                                        ),
                                    });
                                    continue;
                                }
                            }
                        }

                        let builtin_tool = BuiltinTool::from_str(&name);

                        // File claim enforcement: deny write/edit if another agent holds the claim
                        if self.config.session.file_claim_enforcement
                            && matches!(
                                builtin_tool,
                                Some(BuiltinTool::WriteFile | BuiltinTool::EditFile)
                            )
                        {
                            if let Some(fp) = args["path"].as_str() {
                                let normed = normalize_path(fp);
                                if let Some((store, ws_id, agent_name)) = self.context_store() {
                                    match store.check_file_claim(ws_id, &normed).await {
                                        Ok(Some(claim)) if claim.agent_name != agent_name => {
                                            self.permission_gate.renderer().tool_denied(&name);
                                            outcomes.push(ToolOutcome::Denied {
                                                index,
                                                id,
                                                message: format!(
                                                    "File '{normed}' is claimed by agent '{}' \
                                                     — release the claim or coordinate before editing.",
                                                    claim.agent_name
                                                ),
                                            });
                                            continue;
                                        }
                                        Err(e) => {
                                            tracing::warn!(path = %normed, "file claim check failed: {e}");
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }

                        // Write discipline guards (skipped in BypassPermissions mode)
                        let skip_guards = self.permission_gate.skip_write_guards();
                        if !skip_guards && matches!(builtin_tool, Some(BuiltinTool::WriteFile)) {
                            if let Some(path) = args["path"].as_str() {
                                let normed = normalize_path(path);

                                // Guard A: Repeated full rewrite — redirect to edit_file
                                if self.files_full_written.contains(&normed) {
                                    self.permission_gate.renderer().tool_denied(&name);
                                    outcomes.push(ToolOutcome::Denied {
                                        index,
                                        id,
                                        message: format!(
                                            "[Write Guard] You already wrote {} this session. \
                                             Use edit_file to make targeted changes — identify exactly what \
                                             needs to change and replace just that string.",
                                            normed
                                        ),
                                    });
                                    continue;
                                }

                                // Guard 0: Auto-activate plan mode when the agent is building a new
                                // feature (3+ distinct new files attempted in one session, plan mode
                                // not yet active). Fires before Guard B so the 3rd file never gets
                                // added to new_file_design_prompted — cleanly redirected to planning.
                                if !std::path::Path::new(path).exists()
                                    && self.new_file_design_prompted.len() >= 2
                                    && self.plan_file.is_none()
                                    && !self.new_feature_plan_triggered
                                {
                                    self.new_feature_plan_triggered = true;
                                    let home =
                                        std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                                    let plan_path = format!(
                                        "{}/.claude/plans/auto-{}.md",
                                        home,
                                        &Uuid::new_v4().to_string()[..8]
                                    );
                                    let _ =
                                        std::fs::create_dir_all(format!("{}/.claude/plans", home));
                                    self.plan_file = Some(plan_path.clone());
                                    let activated = self.setup_plan_mode_guardrails().await;
                                    self.permission_gate.set_plan_file_enforcement(activated);
                                    self.permission_gate.renderer().tool_denied(&name);
                                    outcomes.push(ToolOutcome::Denied {
                                        index,
                                        id,
                                        message: format!(
                                            "[Plan Mode Auto-Activated] You are creating {} new files — this looks like a new feature.\n\
                                             Plan mode is now active. Before writing any more code:\n\
                                             1. Design your interfaces, types, and module structure\n\
                                             2. Write your plan to: {}\n\
                                             3. Only after ExitPlanMode is approved will writes be unblocked.\n\
                                             Use the Write tool to create the plan file, then call ExitPlanMode.",
                                            self.new_file_design_prompted.len() + 1,
                                            plan_path
                                        ),
                                    });
                                    continue;
                                }

                                // Guard B: New file without design confirmation — fires unconditionally
                                if !self.new_file_design_prompted.contains(&normed)
                                    && !std::path::Path::new(path).exists()
                                {
                                    self.new_file_design_prompted.insert(normed.clone());
                                    self.permission_gate.renderer().tool_denied(&name);
                                    outcomes.push(ToolOutcome::Denied {
                                        index,
                                        id,
                                        message: format!(
                                            "[Design Gate] Before creating {}, draft your interface first:\n\
                                             - Key struct fields / type definitions\n\
                                             - Public function signatures\n\
                                             - Any existing utilities to reuse (search first)\n\
                                             Write these as a comment or outline, then call write_file again.",
                                            normed
                                        ),
                                    });
                                    continue;
                                }

                                // Guard C: Deletion detection — warn when a write would remove
                                // named anchors (exports, routes, nav items, Rust fns) that exist
                                // in the current file. Fires once per path; second attempt is allowed.
                                if std::path::Path::new(path).exists()
                                    && !self.guard_c_warned.contains(&normed)
                                {
                                    if let Ok(old_content) = std::fs::read_to_string(path) {
                                        let new_content =
                                            args["content"].as_str().unwrap_or_default();
                                        let old_anchors = extract_named_anchors(&old_content);
                                        let new_anchors = extract_named_anchors(new_content);
                                        let removed: Vec<&String> = old_anchors
                                            .iter()
                                            .filter(|a| !new_anchors.contains(*a))
                                            .collect();
                                        if !removed.is_empty() {
                                            let mut list = removed
                                                .iter()
                                                .map(|s| s.as_str())
                                                .collect::<Vec<_>>();
                                            list.sort_unstable();
                                            let list_str = list.join(", ");
                                            self.guard_c_warned.insert(normed.clone());
                                            self.permission_gate.renderer().tool_denied(&name);
                                            outcomes.push(ToolOutcome::Denied {
                                                index,
                                                id,
                                                message: format!(
                                                    "[Guard C] You are about to remove these named items \
                                                     that existed in {normed}: {list_str}.\n\
                                                     If this is intentional, re-read the file and explicitly \
                                                     confirm the removal in your response before writing."
                                                ),
                                            });
                                            continue;
                                        }
                                    }
                                }
                            }
                        }

                        // Auto-save: flush IDE buffers to disk before file operations
                        if let Err(msg) = self.hook_runner.run_auto_save(&name, &args).await {
                            self.permission_gate.renderer().tool_denied(&name);
                            outcomes.push(ToolOutcome::Denied {
                                index,
                                id,
                                message: msg,
                            });
                            continue;
                        }

                        // Staleness guard: deny write/edit if the file was modified since last read
                        if !skip_guards && matches!(
                            builtin_tool,
                            Some(BuiltinTool::WriteFile | BuiltinTool::EditFile)
                        ) {
                            if let Some(path) = args["path"].as_str() {
                                let normed = normalize_path(path);
                                if let Some(&read_mtime) = self.files_read_mtime.get(&normed) {
                                    if let Ok(meta) = std::fs::metadata(&normed) {
                                        if let Ok(current_mtime) = meta.modified() {
                                            if current_mtime != read_mtime {
                                                self.permission_gate.renderer().tool_denied(&name);
                                                outcomes.push(ToolOutcome::Denied {
                                                    index,
                                                    id,
                                                    message: format!(
                                                        "Warning: {normed} was modified since last read \
                                                         — re-read before editing."
                                                    ),
                                                });
                                                continue;
                                            }
                                        }
                                    }
                                }
                            }
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
                            Some(BuiltinTool::AskHuman) => {
                                let t0 = std::time::Instant::now();
                                let result = self.handle_ask_human(&ptc.args).await;
                                outcomes.push(ToolOutcome::Result {
                                    index: ptc.index,
                                    id: ptc.id,
                                    name: ptc.name,
                                    args: ptc.args,
                                    result,
                                    elapsed_ms: t0.elapsed().as_millis(),
                                });
                            }
                            Some(BuiltinTool::ReportBlocker) => {
                                let t0 = std::time::Instant::now();
                                let result = self.handle_report_blocker(&ptc.args).await;
                                outcomes.push(ToolOutcome::Result {
                                    index: ptc.index,
                                    id: ptc.id,
                                    name: ptc.name,
                                    args: ptc.args,
                                    result,
                                    elapsed_ms: t0.elapsed().as_millis(),
                                });
                            }
                            Some(BuiltinTool::ReportFinding) => {
                                let t0 = std::time::Instant::now();
                                let result = self.handle_report_finding(&ptc.args).await;
                                outcomes.push(ToolOutcome::Result {
                                    index: ptc.index,
                                    id: ptc.id,
                                    name: ptc.name,
                                    args: ptc.args,
                                    result,
                                    elapsed_ms: t0.elapsed().as_millis(),
                                });
                            }
                            Some(BuiltinTool::RequestReview) => {
                                let t0 = std::time::Instant::now();
                                let result = self.handle_request_review(&ptc.args).await;
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
                                        &[],
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

                                // Create a context handoff record before spawning so the
                                // delegation graph is visible to the dashboard.
                                let handoff_id =
                                    self.create_handoff_for_spawn(&task_str, &ptc.args).await;
                                let spawn_store = self.memory.store().cloned();
                                let spawn_ws_id = self.memory.workspace_id();

                                if run_in_background {
                                    let task_id =
                                        format!("bg_{}", &Uuid::new_v4().to_string()[..8]);
                                    let url = gateway_url.clone();
                                    let key = gateway_key.clone();
                                    let mut spawn_cfg =
                                        spawn::SpawnConfig::from_args(&ptc.args, task_str.clone());
                                    spawn_cfg.handoff_id = handoff_id;

                                    match self.background.spawn_task(
                                        task_id.clone(),
                                        task_str.clone(),
                                        async move {
                                            let result =
                                                match spawn::spawn_agent(spawn_cfg, &url, &key)
                                                    .await
                                                {
                                                    Ok(r) => r,
                                                    Err(e) => spawn::AgentResult {
                                                        status: "error".to_string(),
                                                        summary: e.to_string(),
                                                        cost: 0.0,
                                                        turns: 0,
                                                    },
                                                };
                                            // Complete the handoff in context store
                                            if let (Some(store), Some(ws_id), Some(hid)) =
                                                (spawn_store, spawn_ws_id, handoff_id)
                                            {
                                                use prism_context::store::Store;
                                                let _ = store
                                                    .complete_handoff(
                                                        ws_id,
                                                        hid,
                                                        serde_json::to_value(&result)
                                                            .unwrap_or_default(),
                                                    )
                                                    .await;
                                            }
                                            result
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
                                                    "handoff_id": handoff_id.map(|h| h.to_string()),
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
                                    let mut spawn_cfg =
                                        spawn::SpawnConfig::from_args(&args, task_str);
                                    spawn_cfg.handoff_id = handoff_id;
                                    joinset_pending.push((index, id.clone()));
                                    joinset.spawn(async move {
                                        let t0 = std::time::Instant::now();
                                        let (result_str, agent_result) =
                                            match spawn::spawn_agent(spawn_cfg, &url, &key).await {
                                                Ok(r) => {
                                                    let s = serde_json::to_string(&r)
                                                        .unwrap_or_else(|_| r.summary.clone());
                                                    (s, Some(r))
                                                }
                                                Err(e) => (
                                                    serde_json::json!({
                                                        "status": "error",
                                                        "summary": e.to_string()
                                                    })
                                                    .to_string(),
                                                    None,
                                                ),
                                            };
                                        // Complete the handoff in context store
                                        if let (Some(store), Some(ws_id), Some(hid)) =
                                            (spawn_store, spawn_ws_id, handoff_id)
                                        {
                                            use prism_context::store::Store;
                                            let result_val = agent_result
                                                .as_ref()
                                                .and_then(|r| serde_json::to_value(r).ok())
                                                .unwrap_or_else(
                                                    || serde_json::json!({"status": "error"}),
                                                );
                                            let _ = store
                                                .complete_handoff(ws_id, hid, result_val)
                                                .await;
                                        }
                                        ToolOutcome::Result {
                                            index,
                                            id,
                                            name,
                                            args,
                                            result: result_str,
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
                                let extra_dirs = self.additional_dirs.clone();
                                let cache = Arc::clone(&self.tool_cache);
                                joinset_pending.push((index, id.clone()));
                                joinset.spawn(async move {
                                    let t0 = std::time::Instant::now();
                                    let tool_result = tools::dispatch_cached(
                                        &name,
                                        &args,
                                        &cfg,
                                        Some(&cwd),
                                        mcp.as_deref(),
                                        &extra_dirs,
                                        &cache,
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
                    // Collect bash/run_command outputs for environment error classification.
                    let mut bash_results: Vec<String> = Vec::new();

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

                                // Collect bash/run_command outputs for env error classification.
                                let builtin = BuiltinTool::from_str(&name);
                                if matches!(
                                    builtin,
                                    Some(BuiltinTool::Bash | BuiltinTool::RunCommand)
                                ) {
                                    bash_results.push(result.clone());
                                }

                                // Track files mutated by write/edit tools
                                if matches!(
                                    builtin,
                                    Some(BuiltinTool::WriteFile | BuiltinTool::EditFile)
                                ) && let Some(path) = args["path"].as_str()
                                {
                                    let normed = normalize_path(path);
                                    self.files_touched.insert(normed.clone());
                                    // Track full writes separately for repeat-rewrite guard
                                    if matches!(builtin, Some(BuiltinTool::WriteFile)) {
                                        self.files_full_written.insert(normed.clone());
                                    }
                                    // Refresh mtime after write/edit so subsequent edits don't false-positive
                                    if let Ok(meta) = std::fs::metadata(&normed) {
                                        if let Ok(mtime) = meta.modified() {
                                            self.files_read_mtime.insert(normed.clone(), mtime);
                                        }
                                    }
                                    // Invalidate cached reads for the written file and any
                                    // glob/grep entries whose search root covers this path.
                                    self.tool_cache.invalidate_path(&normed);
                                    self.tool_cache.invalidate_dir_containing(&normed);
                                    if let Some(nudge) = write_coalesce.record_write(&normed, turns)
                                    {
                                        tracing::info!(path = %normed, "write coalesce nudge triggered");
                                        self.permission_gate.renderer().write_coalesce_nudge();
                                        self.session.push_message(common::user_message(nudge));
                                    }
                                    turn_files.push(normed.clone());

                                    // Auto-claim the file on successful write/edit (fire-and-forget)
                                    if self.config.session.file_claim_enforcement {
                                        if let Some((store, ws_id)) = self.store_context() {
                                            let agent_name = crate::config::agent_name_from_env();
                                            tokio::spawn(async move {
                                                use prism_context::store::Store as _;
                                                match store
                                                    .claim_file(ws_id, &agent_name, &normed, Some(FILE_CLAIM_TTL_SECS))
                                                    .await
                                                {
                                                    Ok(_) => {}
                                                    Err(prism_context::error::Error::Conflict(_)) => {}
                                                    Err(e) => tracing::warn!(
                                                        path = %normed,
                                                        "auto-claim after write failed: {e}"
                                                    ),
                                                }
                                            });
                                        }
                                    }
                                }

                                // Record mtime on successful read_file for later staleness checks
                                if matches!(builtin, Some(BuiltinTool::ReadFile)) {
                                    if let Some(path) = args["path"].as_str() {
                                        let normed = normalize_path(path);
                                        if let Ok(meta) = std::fs::metadata(&normed) {
                                            if let Ok(mtime) = meta.modified() {
                                                self.files_read_mtime.insert(normed, mtime);
                                            }
                                        }
                                    }
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

                    // Environment error classifier: when bash/run_command output contains
                    // environment-level TS errors (missing @types etc.), inject a note so the
                    // agent doesn't spin trying to fix them via source edits.
                    let env_error_detected = bash_results
                        .iter()
                        .any(|r| classify_ts_errors(r) == ErrorClass::Environment);
                    if env_error_detected {
                        tracing::info!("environment TS errors detected; injecting classifier note");
                        self.session.push_message(common::user_message(
                            "[Note: The above TypeScript errors are environment/configuration issues \
                             (missing @types packages or tsconfig settings). They cannot be fixed by \
                             editing source files. They do not indicate bugs in your implementation. \
                             Continue with the task — do not attempt to fix these errors.]"
                                .to_string(),
                        ));
                    }

                    // Self-review trigger: if files were written this turn and the assistant text
                    // contains completion signals, schedule a self-review prompt for the next turn.
                    if !turn_files.is_empty() && has_completion_signals(content_buf.trim()) {
                        self.pending_self_review = Some(turn_files.clone());
                        tracing::info!(files = ?turn_files, "scheduling self-review nudge");
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

                    // Verbosity guardrail: nudge after N consecutive turns of long text with no tool calls
                    if let Some(nudge) =
                        verbosity_tracker.record_turn(content_buf.trim().len(), /* had_tool_calls */ true)
                    {
                        tracing::info!("verbosity nudge triggered");
                        self.permission_gate.renderer().verbosity_nudge();
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

        let (cache_hits, cache_misses) = self.tool_cache.stats();
        tracing::debug!(cache_hits, cache_misses, "tool result cache stats for session");

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
        if let Some((store, ws_id, agent_name)) = self.context_store() {
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

        // Emit session-end inbox entries
        let thread_ref_id = self.current_thread_id().await;
        if stop_reason == Some(AgentStopReason::Stop) && !self.config.session.await_review {
            let last_assistant_summary = self
                .session
                .active_messages()
                .iter()
                .rev()
                .find(|m| m.role == prism_types::MessageRole::Assistant)
                .and_then(|m| m.content.as_ref().and_then(|c| c.as_str()))
                .map(|s| crate::common::truncate_with_ellipsis(s, 300))
                .unwrap_or_default();
            let completed_title = format!(
                "Session completed: {} turn{}, ${:.4}",
                turns,
                if turns == 1 { "" } else { "s" },
                total_cost_usd,
            );
            let completed_body = serde_json::json!({
                "task_name": &completed_title,
                "description": "",
                "branch": crate::config::agent_name_from_env(),
                "diff_preview": "",
                "session_cost_usd": total_cost_usd,
                "test_summary": serde_json::Value::Null,
                "files_touched": &files_vec,
                "summary": last_assistant_summary,
            })
            .to_string();
            self.create_inbox_event(
                prism_context::model::InboxEntryType::Completed,
                &completed_title,
                &completed_body,
                prism_context::model::InboxSeverity::Info,
                "thread",
                thread_ref_id,
            )
            .await;
        }

        // Suggest refactor review if many files were touched (fires unconditionally)
        if stop_reason == Some(AgentStopReason::Stop) && files_vec.len() >= 3 {
            self.create_inbox_event(
                prism_context::model::InboxEntryType::Suggestion,
                &format!("{} files modified — consider review", files_vec.len()),
                &format!("Files: {}", files_vec.join(", ")),
                prism_context::model::InboxSeverity::Info,
                "thread",
                thread_ref_id,
            )
            .await;
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

    /// Returns cloned (store, workspace_id) for use by callers outside the agent loop (e.g. REPL).
    pub fn store_context(
        &self,
    ) -> Option<(
        std::sync::Arc<prism_context::store::sqlite::SqliteStore>,
        uuid::Uuid,
    )> {
        let store = self.memory.store()?.clone();
        let ws_id = self.memory.workspace_id()?;
        Some((store, ws_id))
    }

    /// Resolve a thread UUID from an explicit arg or the current thread fallback.
    /// Returns `None` if no thread name is available or the lookup fails.
    async fn resolve_thread_id(
        &self,
        thread_arg: &serde_json::Value,
        store: &dyn prism_context::store::Store,
        ws_id: uuid::Uuid,
    ) -> Option<uuid::Uuid> {
        let name = thread_arg
            .as_str()
            .filter(|s| !s.is_empty())
            .or(self.current_thread.as_deref())?;
        match store.get_thread(ws_id, name).await {
            Ok(t) => Some(t.id),
            Err(e) => {
                tracing::warn!(thread = %name, "thread lookup failed: {e}");
                None
            }
        }
    }

    /// Returns (store, workspace_id, agent_name) if context store is configured.
    fn context_store(&self) -> Option<(&dyn prism_context::store::Store, uuid::Uuid, String)> {
        let store = self.memory.store()?;
        let ws_id = self.memory.workspace_id()?;
        let agent_name = crate::config::agent_name_from_env();
        Some((store.as_ref(), ws_id, agent_name))
    }

    /// Fetch model pricing from the gateway `/v1/models` endpoint and populate the cache.
    /// Always marks `pricing_cache_populated = true` so we never retry on failure.
    async fn populate_pricing_cache(&mut self) {
        self.pricing_cache_populated = true;
        match self.client.list_models().await {
            Ok(resp) => {
                for m in resp.data {
                    let input = m.prism_input_cost_per_1m.unwrap_or(0.0);
                    let output = m.prism_output_cost_per_1m.unwrap_or(0.0);
                    if input > 0.0 || output > 0.0 {
                        self.pricing_cache.insert(m.id, (input, output));
                    }
                }
            }
            Err(e) => {
                tracing::debug!("pricing cache: gateway unavailable, using fallback rates: {e}");
            }
        }
    }

    /// Send heartbeat and set agent state to Working via context store.
    async fn send_heartbeat_and_set_working(&mut self, turn: u32) {
        use prism_context::model::AgentState;
        use prism_context::store::Store as _;

        // Clone the Arc so we hold no borrow on self — allows mutation below.
        let ws_id = match self.memory.workspace_id() {
            Some(id) => id,
            None => return,
        };
        let store = match self.memory.store().cloned() {
            Some(s) => s,
            None => return,
        };
        let agent_name = crate::config::agent_name_from_env();

        let _ = store.heartbeat(ws_id, &agent_name).await;
        self.set_agent_state(AgentState::Working).await;

        // Every 5 turns: persist session state, but only when files_touched changed.
        if turn > 0 && turn % 5 == 0 {
            let current_len = self.files_touched.len();
            if current_len != self.last_checkpoint_file_count {
                let files: Vec<String> = self.files_touched.iter().cloned().collect();
                let _ = store
                    .update_session(ws_id, &agent_name, "in-progress", files)
                    .await;
                self.last_checkpoint_file_count = current_len;
            }
        }
    }

    /// Set agent state in the context store. No-op if store is unavailable.
    async fn set_agent_state(&self, state: prism_context::model::AgentState) {
        use prism_context::store::Store as _;
        let Some(store) = self.memory.store().cloned() else {
            return;
        };
        let Some(ws_id) = self.memory.workspace_id() else {
            return;
        };
        let agent_name = crate::config::agent_name_from_env();
        let _ = store.set_agent_state(ws_id, &agent_name, state).await;
    }

    /// Create an Approval inbox entry for the await-review gate. Returns the entry UUID, or
    /// None if the context store is unavailable.
    async fn create_review_approval_entry(&self) -> Option<uuid::Uuid> {
        use prism_context::model::{InboxEntryType, InboxSeverity};
        use prism_context::store::Store as _;

        let store = self.memory.store()?.clone();
        let ws_id = self.memory.workspace_id()?;
        let agent_name = crate::config::agent_name_from_env();
        let thread_ref_id = self.current_thread_id().await;

        // Build the same body JSON used for Completed entries
        let last_assistant_summary = self
            .session
            .active_messages()
            .iter()
            .rev()
            .find(|m| m.role == prism_types::MessageRole::Assistant)
            .and_then(|m| m.content.as_ref().and_then(|c| c.as_str()))
            .map(|s| crate::common::truncate_with_ellipsis(s, 300))
            .unwrap_or_default();
        let files_vec: Vec<String> = self.files_touched.iter().cloned().collect();
        let total_cost_usd = self.session.total_cost_usd;
        let turns = self.session.turns;
        let title = format!(
            "Session completed: {} turn{}, ${:.4}",
            turns,
            if turns == 1 { "" } else { "s" },
            total_cost_usd,
        );
        let body = serde_json::json!({
            "task_name": &title,
            "description": "",
            "branch": &agent_name,
            "diff_preview": "",
            "session_cost_usd": total_cost_usd,
            "test_summary": serde_json::Value::Null,
            "files_touched": &files_vec,
            "summary": last_assistant_summary,
        })
        .to_string();

        let ref_type = thread_ref_id.map(|_| "thread");
        match store
            .create_inbox_entry(
                ws_id,
                InboxEntryType::Approval,
                &title,
                &body,
                InboxSeverity::Warning,
                Some(&agent_name),
                ref_type,
                thread_ref_id,
            )
            .await
        {
            Ok(entry) => Some(entry.id),
            Err(e) => {
                tracing::warn!(error = %e, "failed to create review approval entry");
                None
            }
        }
    }

    /// Poll the context store for resolution of the given inbox entry. Returns when resolved or
    /// interrupted. Sends heartbeats every 10s to prevent agent reaping.
    async fn poll_for_review_resolution(&self, entry_id: uuid::Uuid) -> ReviewOutcome {
        use prism_context::store::Store as _;
        use std::sync::atomic::Ordering;

        let Some(store) = self.memory.store().cloned() else {
            return ReviewOutcome::Interrupted;
        };
        let Some(ws_id) = self.memory.workspace_id() else {
            return ReviewOutcome::Interrupted;
        };
        let agent_name = crate::config::agent_name_from_env();

        let start = std::time::Instant::now();
        let mut last_elapsed_print = 0u64;

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;

            if self.interrupted.load(Ordering::Relaxed) {
                return ReviewOutcome::Interrupted;
            }

            let _ = store.heartbeat(ws_id, &agent_name).await;

            let elapsed_secs = start.elapsed().as_secs();
            if elapsed_secs / 60 > last_elapsed_print / 60 {
                last_elapsed_print = elapsed_secs;
                eprintln!("[await-review] Still waiting... ({}m elapsed)", elapsed_secs / 60);
            }

            match store.get_inbox_entry(ws_id, entry_id).await {
                Ok(entry) if entry.resolved => {
                    let resolution_raw = entry.resolution.as_deref().unwrap_or("{}");
                    let resolution: serde_json::Value =
                        serde_json::from_str(resolution_raw).unwrap_or(serde_json::Value::Null);
                    let decision = resolution
                        .get("decision")
                        .and_then(|v| v.as_str())
                        .unwrap_or("approve");
                    match decision {
                        "approve" => return ReviewOutcome::Approved,
                        "request_changes" => {
                            let msg = resolution
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            return ReviewOutcome::ChangesRequested(msg);
                        }
                        "reject" => return ReviewOutcome::Rejected,
                        other => {
                            tracing::warn!(decision = %other, "unknown review decision; treating as approve");
                            return ReviewOutcome::Approved;
                        }
                    }
                }
                Ok(_) => {} // not yet resolved, keep polling
                Err(e) => {
                    tracing::warn!(error = %e, "error polling review entry; retrying");
                }
            }
        }
    }

    /// Poll for pending decision notifications and inject them as a system message.
    async fn poll_and_inject_decisions(&mut self) {
        use prism_context::store::Store;

        // Need an owned store clone because we later mutably borrow self.session
        let Some(store) = self.memory.store().cloned() else {
            return;
        };
        let Some(ws_id) = self.memory.workspace_id() else {
            return;
        };
        let agent_name = crate::config::agent_name_from_env();

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

    /// Poll inbox for unread messages from the IDE or other agents, inject as context.
    async fn poll_and_inject_messages(&mut self) {
        use prism_context::store::Store;

        let Some(store) = self.memory.store().cloned() else {
            return;
        };
        let Some(ws_id) = self.memory.workspace_id() else {
            return;
        };
        let agent_name = crate::config::agent_name_from_env();

        let messages = match store.list_messages(ws_id, &agent_name, true).await {
            Ok(msgs) if !msgs.is_empty() => msgs,
            _ => return,
        };

        let mut msg = String::from("MESSAGES FROM OPERATOR (read and act on these):\n");
        for m in &messages {
            msg.push_str(&format!("- From {}: {}\n", m.from_agent, m.content));
        }
        self.session.push_message(common::user_message(msg));

        let _ = store.mark_messages_read(ws_id, &agent_name).await;
    }

    /// Create a handoff record in the context store before forking a child agent.
    /// Returns the handoff UUID if context store is available, else None.
    async fn create_handoff_for_spawn(
        &self,
        task: &str,
        args: &serde_json::Value,
    ) -> Option<uuid::Uuid> {
        use prism_context::store::Store;

        let store = self.memory.store()?.clone();
        let ws_id = self.memory.workspace_id()?;
        let agent_name = crate::config::agent_name_from_env();

        let thread_id = self.current_thread_id().await;
        let constraints = args["constraints"]
            .as_object()
            .and_then(|_| serde_json::from_value(args["constraints"].clone()).ok())
            .unwrap_or_default();
        let mode = args["handoff_mode"]
            .as_str()
            .and_then(prism_context::model::HandoffMode::from_str)
            .unwrap_or(prism_context::model::HandoffMode::DelegateAndAwait);

        match store
            .create_handoff(ws_id, &agent_name, task, thread_id, constraints, mode)
            .await
        {
            Ok(h) => {
                tracing::debug!(handoff_id = %h.id, "created handoff for spawn");
                Some(h.id)
            }
            Err(e) => {
                tracing::debug!(error = %e, "failed to create handoff (non-fatal)");
                None
            }
        }
    }

    /// Returns the UUID of the current thread, if any.
    async fn current_thread_id(&self) -> Option<uuid::Uuid> {
        use prism_context::store::Store;

        let thread_name = self.current_thread.as_deref()?;
        let store = self.memory.store()?;
        let ws_id = self.memory.workspace_id()?;
        store
            .get_thread(ws_id, thread_name)
            .await
            .ok()
            .map(|t| t.id)
    }

    /// Create an inbox entry in the context store. Silently no-ops if context store is unavailable.
    async fn create_inbox_event(
        &self,
        entry_type: prism_context::model::InboxEntryType,
        title: &str,
        body: &str,
        severity: prism_context::model::InboxSeverity,
        ref_type: &str,
        ref_id: Option<uuid::Uuid>,
    ) {
        use prism_context::store::Store;

        let Some(store) = self.memory.store().cloned() else {
            return;
        };
        let Some(ws_id) = self.memory.workspace_id() else {
            return;
        };
        let agent_name = crate::config::agent_name_from_env();

        let ref_type_opt = if ref_type.is_empty() {
            None
        } else {
            Some(ref_type)
        };

        if let Err(e) = store
            .create_or_update_inbox_entry(
                ws_id,
                entry_type,
                title,
                body,
                severity,
                Some(&agent_name),
                ref_type_opt,
                ref_id,
                None, // default 300s cooldown
            )
            .await
        {
            tracing::debug!(error = %e, "failed to create inbox entry");
        }
    }

    /// Poll unread inbox entries addressed to this agent and inject as context.
    /// Approval entries in non-interactive mode are surfaced so the agent knows to pause.
    async fn poll_and_inject_inbox(&mut self) {
        use prism_context::store::{InboxFilters, Store};

        let Some(store) = self.memory.store().cloned() else {
            return;
        };
        let Some(ws_id) = self.memory.workspace_id() else {
            return;
        };
        let agent_name = crate::config::agent_name_from_env();

        let entries = match store
            .list_inbox_entries(
                ws_id,
                InboxFilters {
                    unread_only: true,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(e) if !e.is_empty() => e,
            _ => return,
        };

        // Only inject entries from other agents or without a source (operator-created)
        let relevant: Vec<_> = entries
            .iter()
            .filter(|e| e.source_agent.as_deref().map_or(true, |a| a != agent_name))
            .collect();
        if relevant.is_empty() {
            return;
        }

        let mut msg = String::from("INBOX NOTIFICATIONS (review and act appropriately):\n");
        for entry in &relevant {
            let from = entry.source_agent.as_deref().unwrap_or("operator");
            msg.push_str(&format!(
                "- [{type}:{severity}] {title}{body}\n",
                r#type = entry.entry_type,
                severity = entry.severity,
                title = entry.title,
                body = if entry.body.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", entry.body)
                },
            ));
            tracing::debug!(from, entry_type = %entry.entry_type, title = %entry.title, "injecting inbox entry");
        }
        self.session.push_message(common::user_message(msg));

        // Mark all as read
        for entry in &relevant {
            let _ = store.mark_inbox_read(ws_id, entry.id).await;
        }
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

    /// Set up context guardrails for structural plan mode enforcement.
    /// Creates a thread (or reuses the current one) and restricts:
    /// - `allowed_files` to the plan file only
    /// - `allowed_tools` to everything except bash/run_command (shell escape vectors)
    ///
    /// Returns true if structural enforcement was activated.
    async fn setup_plan_mode_guardrails(&mut self) -> bool {
        use prism_context::store::Store;

        let plan_file = match self.plan_file.clone() {
            Some(pf) => pf,
            None => return false,
        };

        let store = match self.memory.store().cloned() {
            Some(s) => s,
            None => {
                tracing::warn!(
                    "plan mode: no context store — falling back to prompt-for-everything"
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
        use prism_context::store::Store;

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
        let (store, ws_id, agent_name) = self.context_store()?;

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

    /// Switch the permission mode mid-session (e.g. from a /mode REPL command).
    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permission_gate.set_mode(mode);
    }

    /// Return the current permission mode.
    pub fn permission_mode(&self) -> PermissionMode {
        self.permission_gate.mode()
    }

    /// Build a dynamic REPL prompt reflecting current agent state.
    pub fn build_prompt(&self) -> String {
        let mut segments: Vec<String> = Vec::new();

        if let Some(ref p) = self.config.session.persona {
            segments.push(p.clone());
        }

        if let Some(ref t) = self.current_thread {
            segments.push(t.clone());
        }

        if self.session.total_cost_usd > 0.0 {
            segments.push(format!("${:.4}", self.session.total_cost_usd));
        }

        let mode = self.permission_mode();
        let mode_label = match mode {
            PermissionMode::Default | PermissionMode::Auto => None,
            _ => Some(mode.display_name()),
        };
        let (color_start, color_end) = match mode {
            PermissionMode::AcceptEdits => ("\x1b[34m", "\x1b[0m"),
            PermissionMode::Plan => ("\x1b[33m", "\x1b[0m"),
            PermissionMode::BypassPermissions => ("\x1b[31m", "\x1b[0m"),
            _ => ("", ""),
        };
        if let Some(label) = mode_label {
            segments.push(label.to_string());
        }

        if segments.is_empty() {
            return "> ".to_string();
        }

        format!("{}[{}] ›{} ", color_start, segments.join(" · "), color_end)
    }

    /// Set agent state to Idle (called from REPL at prompt).
    pub async fn set_idle(&self) {
        use prism_context::model::AgentState;

        let Some((store, ws_id, agent_name)) = self.context_store() else {
            return;
        };

        let _ = store
            .set_agent_state(ws_id, &agent_name, AgentState::Idle)
            .await;
    }

    /// Handle the `ask_human` tool — posts a question to the human's inbox in the context store.
    /// The REPL surfaces pending inbox entries before each prompt so the human sees it immediately.
    async fn handle_ask_human(&self, args: &serde_json::Value) -> String {
        use prism_context::model::{InboxEntryType, InboxSeverity};

        let Some((store, ws_id, agent_name)) = self.context_store() else {
            return json!({"error": "no context store available"}).to_string();
        };

        let question = args["question"].as_str().unwrap_or("").to_string();
        if question.is_empty() {
            return json!({"error": "question is required"}).to_string();
        }

        let severity = match args["severity"].as_str() {
            Some("critical") => InboxSeverity::Critical,
            Some("warning") => InboxSeverity::Warning,
            _ => InboxSeverity::Info,
        };

        match store
            .create_inbox_entry(
                ws_id,
                InboxEntryType::Approval,
                "Agent needs input",
                &question,
                severity,
                Some(&agent_name),
                None,
                None,
            )
            .await
        {
            Ok(e) => json!({ "sent": true, "inbox_id": e.id.to_string() }).to_string(),
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        }
    }

    /// Handle the `report_blocker` tool — posts a validated blocker to the inbox with evidence.
    async fn handle_report_blocker(&self, args: &serde_json::Value) -> String {
        use prism_context::model::{InboxEntryType, InboxSeverity};

        let Some((store, ws_id, agent_name)) = self.context_store() else {
            return json!({"error": "no context store available"}).to_string();
        };

        let title = args["title"].as_str().unwrap_or("").to_string();
        let description = args["description"].as_str().unwrap_or("").to_string();
        let initialization_trace = args["initialization_trace"].as_str().unwrap_or("").to_string();
        let reachability = args["reachability"].as_str().unwrap_or("").to_string();
        let alternative_handlers = args["alternative_handlers"].as_str().unwrap_or("").to_string();

        for (field, value) in [
            ("title", &title),
            ("description", &description),
            ("initialization_trace", &initialization_trace),
            ("reachability", &reachability),
            ("alternative_handlers", &alternative_handlers),
        ] {
            if value.is_empty() {
                return json!({"error": format!("{field} is required")}).to_string();
            }
        }

        let severity = match args["severity"].as_str() {
            Some("critical") => InboxSeverity::Critical,
            _ => InboxSeverity::Warning,
        };

        let body = json!({
            "description": description,
            "evidence": {
                "initialization_trace": initialization_trace,
                "reachability": reachability,
                "alternative_handlers": alternative_handlers,
            }
        })
        .to_string();

        let thread_id = self.resolve_thread_id(&args["thread"], store, ws_id).await;
        let ref_type = thread_id.is_some().then_some("thread");

        match store
            .create_inbox_entry(
                ws_id,
                InboxEntryType::Blocked,
                &title,
                &body,
                severity,
                Some(&agent_name),
                ref_type,
                thread_id,
            )
            .await
        {
            Ok(e) => json!({ "reported": true, "inbox_id": e.id.to_string() }).to_string(),
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        }
    }

    /// Handle the `report_finding` tool — posts a code review finding with a graduated confidence level.
    async fn handle_report_finding(&self, args: &serde_json::Value) -> String {
        use prism_context::model::{InboxEntryType, InboxSeverity};

        let Some((store, ws_id, agent_name)) = self.context_store() else {
            return json!({"error": "no context store available"}).to_string();
        };

        let confidence = args["confidence"].as_str().unwrap_or("").to_string();
        let title = args["title"].as_str().unwrap_or("").to_string();
        let description = args["description"].as_str().unwrap_or("").to_string();

        for (field, value) in [("confidence", &confidence), ("title", &title), ("description", &description)] {
            if value.is_empty() {
                return json!({"error": format!("{field} is required")}).to_string();
            }
        }

        let initialization_trace = args["initialization_trace"].as_str().unwrap_or("").to_string();
        let reachability = args["reachability"].as_str().unwrap_or("").to_string();
        let alternative_handlers = args["alternative_handlers"].as_str().unwrap_or("").to_string();

        // Validate graduated evidence requirements
        let (entry_type, severity) = match confidence.as_str() {
            "blocker" => {
                for (field, value) in [
                    ("initialization_trace", &initialization_trace),
                    ("reachability", &reachability),
                    ("alternative_handlers", &alternative_handlers),
                ] {
                    if value.is_empty() {
                        return json!({"error": format!("{field} is required for confidence=blocker")}).to_string();
                    }
                }
                (InboxEntryType::Blocked, InboxSeverity::Critical)
            }
            "likely_blocker" => {
                for (field, value) in [
                    ("initialization_trace", &initialization_trace),
                    ("reachability", &reachability),
                ] {
                    if value.is_empty() {
                        return json!({"error": format!("{field} is required for confidence=likely_blocker")}).to_string();
                    }
                }
                (InboxEntryType::Blocked, InboxSeverity::Warning)
            }
            "concern" => (InboxEntryType::Risk, InboxSeverity::Warning),
            "nit" => (InboxEntryType::Suggestion, InboxSeverity::Info),
            other => return json!({"error": format!("unknown confidence level: {other}")}).to_string(),
        };

        let evidence: Option<serde_json::Value> = {
            let mut m = serde_json::Map::new();
            for (key, val) in [
                ("initialization_trace", &initialization_trace),
                ("reachability", &reachability),
                ("alternative_handlers", &alternative_handlers),
            ] {
                if !val.is_empty() {
                    m.insert(key.to_string(), json!(val));
                }
            }
            if m.is_empty() { None } else { Some(serde_json::Value::Object(m)) }
        };

        let body = match evidence {
            Some(ev) => json!({"confidence": confidence, "description": description, "evidence": ev}),
            None => json!({"confidence": confidence, "description": description}),
        }
        .to_string();

        let thread_id = self.resolve_thread_id(&args["thread"], store, ws_id).await;
        let ref_type = thread_id.is_some().then_some("thread");

        match store
            .create_inbox_entry(
                ws_id,
                entry_type,
                &title,
                &body,
                severity,
                Some(&agent_name),
                ref_type,
                thread_id,
            )
            .await
        {
            Ok(e) => json!({ "reported": true, "inbox_id": e.id.to_string() }).to_string(),
            Err(e) => json!({ "error": e.to_string() }).to_string(),
        }
    }

    /// Handle the `request_review` tool — creates an Approval inbox entry and blocks until resolved.
    async fn handle_request_review(&self, args: &serde_json::Value) -> String {
        use prism_context::model::{AgentState, InboxEntryType, InboxSeverity};

        let Some((store, ws_id, agent_name)) = self.context_store() else {
            return json!({"error": "no context store available"}).to_string();
        };

        let title = args["title"].as_str().unwrap_or("").to_string();
        let body = args["body"].as_str().unwrap_or("").to_string();

        for (field, value) in [("title", &title), ("body", &body)] {
            if value.is_empty() {
                return json!({"error": format!("{field} is required")}).to_string();
            }
        }

        let severity = match args["severity"].as_str() {
            Some("critical") => InboxSeverity::Critical,
            Some("info") => InboxSeverity::Info,
            _ => InboxSeverity::Warning,
        };

        let thread_id = self.resolve_thread_id(&args["thread"], store, ws_id).await;
        let ref_type = thread_id.is_some().then_some("thread");

        // Serialize body as structured JSON so the Zed modal can extract fields.
        let body_json = json!({
            "description": body,
            "diff_preview": args["diff_preview"].as_str().unwrap_or(""),
            "branch": args["branch"].as_str().unwrap_or(""),
            "session_cost_usd": args["session_cost_usd"].as_f64(),
            "test_summary": args["test_summary"].as_str(),
        });
        let body_str = body_json.to_string();

        let entry = match store
            .create_inbox_entry(
                ws_id,
                InboxEntryType::Approval,
                &title,
                &body_str,
                severity,
                Some(&agent_name),
                ref_type,
                thread_id,
            )
            .await
        {
            Ok(e) => e,
            Err(e) => return json!({"error": e.to_string()}).to_string(),
        };

        const POLL_INTERVAL_SECS: u64 = 3;
        const TIMEOUT_SECS: u64 = 3600; // 1 hour

        let inbox_id = entry.id;
        let inbox_id_str = inbox_id.to_string();
        eprintln!(
            "\n[request_review] Parked — waiting for human approval.\n  Inbox entry: {inbox_id_str}\n  Run: prism context inbox resolve {inbox_id_str} --response \"approved\"\n"
        );

        let _ = store.set_agent_state(ws_id, &agent_name, AgentState::Blocked).await;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(TIMEOUT_SECS);
        loop {
            if self.interrupted.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = store.set_agent_state(ws_id, &agent_name, AgentState::Idle).await;
                return json!({
                    "status": "interrupted",
                    "inbox_id": inbox_id_str,
                    "message": "request_review interrupted by user before resolution"
                })
                .to_string();
            }

            if std::time::Instant::now() >= deadline {
                let _ = store.set_agent_state(ws_id, &agent_name, AgentState::Idle).await;
                return json!({
                    "status": "timeout",
                    "inbox_id": inbox_id_str,
                    "message": format!("request_review timed out after {} hour with no human response", TIMEOUT_SECS / 3600)
                })
                .to_string();
            }

            tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;

            match store.get_inbox_entry(ws_id, inbox_id).await {
                Ok(e) if e.resolved => {
                    let _ = store.set_agent_state(ws_id, &agent_name, AgentState::Working).await;
                    let resolution_raw = e.resolution.as_deref().unwrap_or("{}");
                    // Try to parse structured JSON; fall back to treating raw string as decision.
                    let resolution_json: serde_json::Value = serde_json::from_str(resolution_raw)
                        .unwrap_or_else(|_| json!({"decision": resolution_raw}));
                    return json!({
                        "status": "resolved",
                        "inbox_id": inbox_id_str,
                        "decision": resolution_json.get("decision").and_then(|v| v.as_str()).unwrap_or("unknown"),
                        "message": resolution_json.get("message").and_then(|v| v.as_str()),
                    })
                    .to_string();
                }
                Ok(_) => continue,
                Err(e) => {
                    return json!({"error": format!("polling failed: {e}")}).to_string();
                }
            }
        }
    }

    /// Handle the `record_decision` tool — persists a decision with rationale to the context store.
    async fn handle_record_decision(&self, args: &serde_json::Value) -> String {
        use prism_context::model::DecisionScope;

        let Some((store, ws_id, _agent_name)) = self.context_store() else {
            return json!({"error": "no context store available"}).to_string();
        };

        let title = args["title"].as_str().unwrap_or("").to_string();
        let content = args["content"].as_str().unwrap_or("").to_string();
        if title.is_empty() {
            return json!({"error": "title is required"}).to_string();
        }

        let thread_id = self.resolve_thread_id(&args["thread"], store, ws_id).await;

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

    /// Handle the `recall` tool — loads context from the context store.
    async fn handle_recall(&self, args: &serde_json::Value) -> String {
        use prism_context::store::Store;

        let Some(store) = self.memory.store() else {
            return json!({"error": "no context store available"}).to_string();
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
    prism_context::util::parse_duration(s).map_err(|e| anyhow::anyhow!(e))
}

fn make_guardrails(
    ws_id: uuid::Uuid,
    allowed_files: Vec<String>,
    allowed_tools: Vec<String>,
) -> prism_context::model::ThreadGuardrails {
    prism_context::model::ThreadGuardrails {
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

/// Hardcoded cost fallback rates (per 1M tokens) when gateway pricing is unavailable.
fn fallback_cost_rate(model: &str) -> (f64, f64) {
    match model {
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
    }
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
    fn verbosity_tracker_fires_at_threshold() {
        let mut tracker = VerbosityTracker::new(100, 3);
        // Short text — no increment
        assert!(tracker.record_turn(50, false).is_none());
        // Verbose turns
        assert!(tracker.record_turn(200, false).is_none());
        assert!(tracker.record_turn(200, false).is_none());
        let nudge = tracker.record_turn(200, false);
        assert!(nudge.is_some());
        assert!(nudge.unwrap().contains("Verbosity Check"));
    }

    #[test]
    fn verbosity_tracker_fires_only_once_per_streak() {
        let mut tracker = VerbosityTracker::new(100, 2);
        tracker.record_turn(200, false);
        let first = tracker.record_turn(200, false);
        assert!(first.is_some());
        // Continues verbose — no second nudge
        let second = tracker.record_turn(200, false);
        assert!(second.is_none());
    }

    #[test]
    fn verbosity_tracker_resets_on_execution_turn() {
        let mut tracker = VerbosityTracker::new(100, 2);
        tracker.record_turn(200, false);
        tracker.record_turn(200, false); // fires nudge
        // Execution turn: tool calls + short text resets streak
        tracker.record_turn(50, true);
        assert!(tracker.record_turn(200, false).is_none());
        // Fires again after a full new streak
        let nudge = tracker.record_turn(200, false);
        assert!(nudge.is_some());
    }

    #[test]
    fn verbosity_tracker_disabled_when_threshold_zero() {
        let mut tracker = VerbosityTracker::new(100, 0);
        for _ in 0..20 {
            assert!(tracker.record_turn(9999, false).is_none());
        }
    }

    #[test]
    fn verbosity_tracker_verbose_with_tool_calls_increments_streak() {
        // Long text alongside tool calls still counts as verbose
        let mut tracker = VerbosityTracker::new(100, 2);
        tracker.record_turn(200, true); // verbose + tool calls
        let nudge = tracker.record_turn(200, true);
        assert!(nudge.is_some());
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
