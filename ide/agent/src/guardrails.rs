use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Returns true if the user's prompt contains signals that a decision is required
/// before implementation.
pub fn has_decision_signals(text: &str) -> bool {
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

/// Returns true if the assistant text contains common completion signals.
pub fn has_completion_signals(text: &str) -> bool {
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

/// Extract "named anchors" from file content: exports, routes, nav links, and top-level Rust fns.
/// Used by Guard C to detect when a write would silently remove existing named items.
pub fn extract_named_anchors(content: &str) -> HashSet<String> {
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

/// Returns true for read-only tools that don't modify the file system.
pub fn is_read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "grep"
            | "codebase_search"
            | "find_path"
            | "list_directory"
            | "diagnostics"
            | "now"
            | "recall"
            | "web_search"
            | "fetch"
            | "open"
    )
}

// ---------------------------------------------------------------------------
// ExplorationBudget
// ---------------------------------------------------------------------------

/// Tracks consecutive read-only turns and emits a convergence nudge when the threshold is reached.
pub struct ExplorationBudget {
    pub consecutive_readonly_turns: u32,
    pub threshold: u32,
    /// True after the nudge has fired for the current streak; clears on streak reset.
    pub streak_nudged: bool,
}

impl ExplorationBudget {
    pub fn new(threshold: u32) -> Self {
        Self {
            consecutive_readonly_turns: 0,
            threshold,
            streak_nudged: false,
        }
    }

    /// Record a turn. Returns `Some(nudge message)` if the threshold is reached for the first time
    /// in the current streak. Resets when a write tool is used or the model produces substantive text.
    pub fn record_turn(&mut self, all_readonly: bool, had_substantive_text: bool) -> Option<String> {
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

// ---------------------------------------------------------------------------
// DecisionCheckpoint
// ---------------------------------------------------------------------------

/// Fires a checkpoint message when the agent has explored enough to decide.
pub struct DecisionCheckpoint {
    pub armed: bool,
    pub exploration_count: u32,
    pub fired: bool,
    pub decision_threshold: u32,
    pub question_interval: u32,
    pub last_question_turn: u32,
}

impl DecisionCheckpoint {
    pub fn new(decision_threshold: u32, question_interval: u32) -> Self {
        Self {
            armed: false,
            exploration_count: 0,
            fired: false,
            decision_threshold,
            question_interval,
            last_question_turn: 0,
        }
    }

    /// Arm if the user message contains decision signals.
    pub fn arm_if_decision_prompt(&mut self, text: &str) {
        if has_decision_signals(text) {
            self.armed = true;
            self.fired = false;
            self.exploration_count = 0;
        }
    }

    /// Record an exploration (read-only tool call).
    pub fn record_exploration(&mut self) {
        self.exploration_count += 1;
    }

    /// Returns messages to inject before this turn (decision checkpoint + question checkpoint).
    pub fn pre_turn_messages(&mut self, turn: u32) -> Vec<String> {
        let mut msgs = Vec::new();

        // A) Decision checkpoint: fire once after enough exploration calls
        if self.armed && !self.fired && self.exploration_count >= self.decision_threshold {
            self.fired = true;
            msgs.push(
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
            );
        }

        // B) Question checkpoint: every N turns, remind agent to surface unknowns
        if self.question_interval > 0
            && turn > 0
            && turn % self.question_interval == 0
            && turn != self.last_question_turn
        {
            self.last_question_turn = turn;
            msgs.push(
                "[Question Checkpoint] You've been working for several turns. \
Before continuing, consider:\n\
- Are there ambiguities in the requirements you should ask about?\n\
- Are you making assumptions the user should confirm?\n\
- Is this heading in the direction the user expects?\n\
If you have questions, ask them now. If you're confident, continue."
                    .to_string(),
            );
        }

        msgs
    }
}

// ---------------------------------------------------------------------------
// SelfReview
// ---------------------------------------------------------------------------

/// Schedules a self-review nudge to be injected at the start of the next turn.
pub struct SelfReview {
    pub pending_files: Option<Vec<String>>,
}

impl SelfReview {
    pub fn new() -> Self {
        Self { pending_files: None }
    }

    /// Schedule a review for the given files (to be injected next turn).
    pub fn schedule_review(&mut self, files: Vec<String>) {
        self.pending_files = Some(files);
    }

    /// Take and return the review message if one is pending.
    pub fn take_review_message(&mut self) -> Option<String> {
        self.pending_files.take().map(|files| {
            let file_list = files.join(", ");
            format!(
                "[Self-review] Before declaring complete, verify:\n\
                 1. No existing exports, routes, or nav items were removed from: {file_list}\n\
                 2. The implementation matches the original request\n\
                 3. If you removed anything intentionally, state it explicitly."
            )
        })
    }
}

// ---------------------------------------------------------------------------
// GuardDenial + WriteGuards
// ---------------------------------------------------------------------------

/// Reason a write was denied.
pub enum GuardDenial {
    /// Guard A: file was already fully written this session.
    RepeatRewrite,
    /// Guard B: new file being created without design gate.
    DesignGate,
    /// Guard C: write would remove named anchors.
    AnchorRemoval { removed: Vec<String> },
}

impl GuardDenial {
    pub fn message(&self) -> String {
        match self {
            GuardDenial::RepeatRewrite => {
                "You already wrote this file. Use edit mode for targeted changes.".to_string()
            }
            GuardDenial::DesignGate => {
                "Before creating a new file, draft the interface first (struct fields, \
                 function signatures, key imports). Then try again."
                    .to_string()
            }
            GuardDenial::AnchorRemoval { removed } => {
                format!(
                    "This write would remove: {}. If intentional, try again.",
                    removed.join(", ")
                )
            }
        }
    }
}

/// Combines Guard A (no repeat rewrites), Guard B (design gate), Guard C (anchor removal).
pub struct WriteGuards {
    pub files_full_written: HashSet<String>,
    pub new_file_design_prompted: HashSet<String>,
    pub anchor_warned: HashSet<String>,
}

impl WriteGuards {
    pub fn new() -> Self {
        Self {
            files_full_written: HashSet::new(),
            new_file_design_prompted: HashSet::new(),
            anchor_warned: HashSet::new(),
        }
    }

    /// Check if a write should be denied. Checks A → B → C in order.
    pub fn check_write(
        &mut self,
        path: &str,
        is_new_file: bool,
        old_content: Option<&str>,
        new_content: Option<&str>,
    ) -> Option<GuardDenial> {
        // Guard A: no repeat full rewrites
        if self.files_full_written.contains(path) {
            return Some(GuardDenial::RepeatRewrite);
        }

        // Guard B: design gate for new files
        if is_new_file && !self.new_file_design_prompted.contains(path) {
            self.new_file_design_prompted.insert(path.to_string());
            return Some(GuardDenial::DesignGate);
        }

        // Guard C: anchor removal detection
        if !is_new_file {
            if let (Some(old), Some(new)) = (old_content, new_content) {
                if !self.anchor_warned.contains(path) {
                    let old_anchors = extract_named_anchors(old);
                    let new_anchors = extract_named_anchors(new);
                    let removed: Vec<String> =
                        old_anchors.difference(&new_anchors).cloned().collect();
                    if !removed.is_empty() {
                        self.anchor_warned.insert(path.to_string());
                        return Some(GuardDenial::AnchorRemoval { removed });
                    }
                }
            }
        }

        None
    }

    /// Record that a full write occurred for the given path.
    pub fn record_full_write(&mut self, path: String) {
        self.files_full_written.insert(path);
    }
}

// ---------------------------------------------------------------------------
// VerbosityTracker
// ---------------------------------------------------------------------------

/// Fires a nudge when the agent produces N consecutive long-text turns without tool calls.
pub struct VerbosityTracker {
    pub consecutive_verbose_turns: u32,
    pub char_threshold: usize,
    pub turn_threshold: u32,
    pub streak_nudged: bool,
}

impl VerbosityTracker {
    pub fn new(char_threshold: usize, turn_threshold: u32) -> Self {
        Self {
            consecutive_verbose_turns: 0,
            char_threshold,
            turn_threshold,
            streak_nudged: false,
        }
    }

    /// Record a turn. Returns `Some(nudge)` when threshold is hit for the first time in a streak.
    pub fn record_turn(&mut self, text_len: usize, had_tool_calls: bool) -> Option<String> {
        if self.turn_threshold == 0 {
            return None; // disabled
        }

        if text_len > self.char_threshold {
            self.consecutive_verbose_turns += 1;
        } else if had_tool_calls {
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

// ---------------------------------------------------------------------------
// PlanGuard (Guard 0): Auto-activate plan mode on 3+ new-file attempts
// ---------------------------------------------------------------------------

/// Tracks distinct new-file write attempts. When the count reaches 3 without a plan
/// already being active, auto-activates plan mode and supplies a path for the plan file.
pub struct PlanGuard {
    /// Number of distinct new-file write attempts this session.
    pub new_file_count: usize,
    /// Whether plan mode has already been triggered.
    pub triggered: bool,
    /// Path of the auto-generated plan file (set when triggered).
    pub plan_file: Option<String>,
}

impl PlanGuard {
    pub fn new() -> Self {
        Self {
            new_file_count: 0,
            triggered: false,
            plan_file: None,
        }
    }

    /// Called before every new-file write attempt. Returns `Some(denial)` if plan mode
    /// should be auto-activated (write blocked), `None` if the write may proceed.
    pub fn check_new_file(&mut self) -> Option<String> {
        if self.triggered {
            return None;
        }
        self.new_file_count += 1;
        if self.new_file_count >= 3 {
            self.triggered = true;
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            let plan_path = format!(
                "{}/.claude/plans/auto-{}.md",
                home,
                &uuid::Uuid::new_v4().to_string()[..8]
            );
            let _ = std::fs::create_dir_all(format!("{}/.claude/plans", home));
            self.plan_file = Some(plan_path.clone());
            Some(format!(
                "[Plan Mode Auto-Activated] You are creating {n} new files — this looks like a new feature.\n\
                 Plan mode is now active. Before writing any more code:\n\
                 1. Design your interfaces, types, and module structure\n\
                 2. Write your plan to: {plan_path}\n\
                 3. Only after the plan is written and reviewed will writes be unblocked.\n\
                 Use the Write/edit_file tool to create the plan file first.",
                n = self.new_file_count,
                plan_path = plan_path
            ))
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Guardrails (top-level aggregate)
// ---------------------------------------------------------------------------

/// All guardrails bundled together, owned by Thread.
pub struct Guardrails {
    pub exploration_budget: ExplorationBudget,
    pub decision_checkpoint: DecisionCheckpoint,
    pub self_review: SelfReview,
    pub write_guards: WriteGuards,
    pub verbosity: VerbosityTracker,
    pub plan_guard: PlanGuard,
    pub turn_count: u32,
    turn_files_written: Vec<String>,
    turn_tool_names: Vec<String>,
}

impl Guardrails {
    pub fn new() -> Self {
        Self {
            exploration_budget: ExplorationBudget::new(6),
            decision_checkpoint: DecisionCheckpoint::new(8, 5),
            self_review: SelfReview::new(),
            write_guards: WriteGuards::new(),
            verbosity: VerbosityTracker::new(2000, 3),
            plan_guard: PlanGuard::new(),
            turn_count: 0,
            turn_files_written: Vec::new(),
            turn_tool_names: Vec::new(),
        }
    }

    pub fn increment_turn(&mut self) {
        self.turn_count += 1;
    }

    pub fn reset_turn(&mut self) {
        self.turn_files_written.clear();
        self.turn_tool_names.clear();
    }

    pub fn mark_write(&mut self, path: String) {
        self.turn_files_written.push(path);
    }

    pub fn record_tool(&mut self, name: &str) {
        self.turn_tool_names.push(name.to_string());
    }

    pub fn had_tool_calls(&self) -> bool {
        !self.turn_tool_names.is_empty()
    }

    pub fn take_turn_files(&mut self) -> Vec<String> {
        std::mem::take(&mut self.turn_files_written)
    }

    /// Returns true if all tools used this turn were read-only (no writes).
    pub fn turn_all_readonly(&self) -> bool {
        if self.turn_tool_names.is_empty() {
            return true;
        }
        self.turn_tool_names.iter().all(|n| is_read_only_tool(n))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exploration_budget_nudge() {
        let mut budget = ExplorationBudget::new(6);
        // First 5 readonly turns — no nudge
        for _ in 0..5 {
            assert!(budget.record_turn(true, false).is_none());
        }
        // 6th turn — nudge fires
        assert!(budget.record_turn(true, false).is_some());
        // 7th turn — nudge already fired for this streak, no repeat
        assert!(budget.record_turn(true, false).is_none());
        // Write turn resets streak
        assert!(budget.record_turn(false, false).is_none());
        assert_eq!(budget.consecutive_readonly_turns, 0);
        // 6 more readonly turns — nudge fires again
        for _ in 0..5 {
            assert!(budget.record_turn(true, false).is_none());
        }
        assert!(budget.record_turn(true, false).is_some());
    }

    #[test]
    fn test_guard_a_repeat_rewrite() {
        let mut guards = WriteGuards::new();
        // First write to a new file — design gate fires (Guard B), write denied, no record
        let denial = guards.check_write("/foo/bar.rs", true, None, Some("fn main() {}"));
        assert!(matches!(denial, Some(GuardDenial::DesignGate)));

        // Second attempt — design gate already prompted, allowed this time
        let denial = guards.check_write("/foo/bar.rs", false, Some("fn main() {}"), Some("fn main() { println!(\"hi\"); }"));
        assert!(denial.is_none());
        // Record the successful write
        guards.record_full_write("/foo/bar.rs".to_string());

        // Third write to same file — Guard A fires (repeat rewrite)
        let denial = guards.check_write("/foo/bar.rs", false, None, None);
        assert!(matches!(denial, Some(GuardDenial::RepeatRewrite)));
    }

    #[test]
    fn test_guard_b_design_gate() {
        let mut guards = WriteGuards::new();
        // First write to a new file — denied (design gate)
        let denial = guards.check_write("/new/file.rs", true, None, Some("content"));
        assert!(matches!(denial, Some(GuardDenial::DesignGate)));
        // Second attempt — allowed
        let denial = guards.check_write("/new/file.rs", true, None, Some("content"));
        assert!(denial.is_none());
    }

    #[test]
    fn test_guard_c_anchor_removal() {
        let mut guards = WriteGuards::new();
        let old = "pub fn foo() {}\npub fn bar() {}";
        let new = "pub fn bar() {}";
        // First write removing `fn foo` — denied
        let denial = guards.check_write("/src/lib.rs", false, Some(old), Some(new));
        assert!(matches!(denial, Some(GuardDenial::AnchorRemoval { .. })));
        // Second attempt — allowed (anchor_warned set)
        let denial = guards.check_write("/src/lib.rs", false, Some(old), Some(new));
        assert!(denial.is_none());
    }

    #[test]
    fn test_decision_checkpoint() {
        let mut cp = DecisionCheckpoint::new(8, 0); // disable question interval
        // Not armed — no messages
        for _ in 0..10 {
            cp.record_exploration();
        }
        assert!(cp.pre_turn_messages(1).is_empty());

        // Arm
        cp.arm_if_decision_prompt("decide between option a and option b");
        assert!(cp.armed);

        // Only 0 explorations recorded since arm — need 8 more
        cp.exploration_count = 0;
        for _ in 0..7 {
            cp.record_exploration();
        }
        assert!(cp.pre_turn_messages(2).is_empty());

        // 8th exploration — fires
        cp.record_exploration();
        let msgs = cp.pre_turn_messages(3);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].contains("Decision Checkpoint"));

        // Does not fire again
        cp.record_exploration();
        let msgs = cp.pre_turn_messages(4);
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_self_review_trigger() {
        let mut sr = SelfReview::new();
        // No pending review initially
        assert!(sr.take_review_message().is_none());

        // Schedule review
        sr.schedule_review(vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]);
        let msg = sr.take_review_message();
        assert!(msg.is_some());
        assert!(msg.unwrap().contains("src/lib.rs"));

        // After taking, none pending
        assert!(sr.take_review_message().is_none());
    }

    #[test]
    fn test_verbosity_tracker() {
        let mut vt = VerbosityTracker::new(2000, 3);
        // Two verbose turns — no nudge
        assert!(vt.record_turn(3000, false).is_none());
        assert!(vt.record_turn(3000, false).is_none());
        // Third verbose turn — nudge fires
        assert!(vt.record_turn(3000, false).is_some());
        // Fourth — no repeat
        assert!(vt.record_turn(3000, false).is_none());
        // Tool-call turn with short text resets
        assert!(vt.record_turn(100, true).is_none());
        assert_eq!(vt.consecutive_verbose_turns, 0);
    }
}
