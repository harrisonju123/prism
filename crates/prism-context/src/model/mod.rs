use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Thread status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadStatus {
    Active,
    Archived,
}

impl std::fmt::Display for ThreadStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThreadStatus::Active => write!(f, "active"),
            ThreadStatus::Archived => write!(f, "archived"),
        }
    }
}

// ---------------------------------------------------------------------------
// Agent state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Dead,
    AwaitingReview,
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentState::Idle => write!(f, "idle"),
            AgentState::Working => write!(f, "working"),
            AgentState::Blocked => write!(f, "blocked"),
            AgentState::Dead => write!(f, "dead"),
            AgentState::AwaitingReview => write!(f, "awaiting_review"),
        }
    }
}

impl AgentState {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "idle" => Some(Self::Idle),
            "working" => Some(Self::Working),
            "blocked" => Some(Self::Blocked),
            "dead" => Some(Self::Dead),
            "awaiting_review" => Some(Self::AwaitingReview),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Decision status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Active,
    Superseded,
    Revoked,
}

impl std::fmt::Display for DecisionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecisionStatus::Active => write!(f, "active"),
            DecisionStatus::Superseded => write!(f, "superseded"),
            DecisionStatus::Revoked => write!(f, "revoked"),
        }
    }
}

// ---------------------------------------------------------------------------
// Decision scope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionScope {
    Thread,
    Workspace,
}

impl std::fmt::Display for DecisionScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecisionScope::Thread => write!(f, "thread"),
            DecisionScope::Workspace => write!(f, "workspace"),
        }
    }
}

impl DecisionScope {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "thread" => Some(Self::Thread),
            "workspace" => Some(Self::Workspace),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Handoff mode + status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffMode {
    DelegateAndAwait,
    DelegateAndForget,
}

impl std::fmt::Display for HandoffMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandoffMode::DelegateAndAwait => write!(f, "delegate_and_await"),
            HandoffMode::DelegateAndForget => write!(f, "delegate_and_forget"),
        }
    }
}

impl HandoffMode {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "delegate_and_await" => Some(Self::DelegateAndAwait),
            "delegate_and_forget" => Some(Self::DelegateAndForget),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HandoffStatus {
    Pending,
    Accepted,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for HandoffStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandoffStatus::Pending => write!(f, "pending"),
            HandoffStatus::Accepted => write!(f, "accepted"),
            HandoffStatus::Running => write!(f, "running"),
            HandoffStatus::Completed => write!(f, "completed"),
            HandoffStatus::Failed => write!(f, "failed"),
            HandoffStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl HandoffStatus {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Inbox entry types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxEntryType {
    Approval,
    Blocked,
    Suggestion,
    Risk,
    CostSpike,
    Completed,
}

impl std::fmt::Display for InboxEntryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Approval => write!(f, "approval"),
            Self::Blocked => write!(f, "blocked"),
            Self::Suggestion => write!(f, "suggestion"),
            Self::Risk => write!(f, "risk"),
            Self::CostSpike => write!(f, "cost_spike"),
            Self::Completed => write!(f, "completed"),
        }
    }
}

impl InboxEntryType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "approval" => Some(Self::Approval),
            "blocked" => Some(Self::Blocked),
            "suggestion" => Some(Self::Suggestion),
            "risk" => Some(Self::Risk),
            "cost_spike" => Some(Self::CostSpike),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InboxSeverity {
    Critical,
    Warning,
    Info,
}

impl std::fmt::Display for InboxSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Critical => write!(f, "critical"),
            Self::Warning => write!(f, "warning"),
            Self::Info => write!(f, "info"),
        }
    }
}

impl InboxSeverity {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "critical" => Some(Self::Critical),
            "warning" => Some(Self::Warning),
            "info" => Some(Self::Info),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Core entities
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub status: ThreadStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub access_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A supervisory feed item surfaced by agents to request human review or inform of status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxEntry {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub entry_type: InboxEntryType,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    pub severity: InboxSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_agent: Option<String>,
    /// Entity type this entry links to (e.g. "thread", "handoff", "decision").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_type: Option<String>,
    /// UUID of the referenced entity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<Uuid>,
    pub read: bool,
    pub dismissed: bool,
    pub resolved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    pub status: DecisionStatus,
    pub scope: DecisionScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub state: AgentState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_thread_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checkin: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub branch: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub worktree_path: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_touched: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub branch: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub worktree_path: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor: String,
    pub action: String,
    pub entity_type: String,
    pub entity_id: Uuid,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    pub content: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub from_agent: String,
    pub to_agent: String,
    pub content: String,
    pub read: bool,
    pub created_at: DateTime<Utc>,
    /// Groups all messages belonging to a single task conversation (initial request + Q&A).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Composite read types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub name: String,
    pub state: AgentState,
    pub session_open: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checkin: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadContext {
    pub thread: Thread,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memories: Vec<Memory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_sessions: Vec<AgentSession>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_activity: Vec<ActivityEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckinContext {
    pub agent: Agent,
    pub session: AgentSession,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_threads: Vec<Thread>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub global_memories: Vec<Memory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_sessions: Vec<AgentSession>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_agents: Vec<AgentStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_decisions: Vec<Decision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_handoffs: Vec<Handoff>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResult {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memories: Vec<Memory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity: Vec<ActivityEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceOverview {
    pub workspace: Workspace,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_threads: Vec<Thread>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_memories: Vec<Memory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_decisions: Vec<Decision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_agents: Vec<AgentStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_sessions: Vec<AgentSession>,
}

// ---------------------------------------------------------------------------
// Handoff
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HandoffConstraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handoff {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub from_agent_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_agent_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
    pub task: String,
    pub constraints: HandoffConstraints,
    pub mode: HandoffMode,
    pub status: HandoffStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Plan + WorkPackage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanStatus {
    Draft,
    Approved,
    Active,
    Completed,
    Cancelled,
}

impl std::fmt::Display for PlanStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanStatus::Draft => write!(f, "draft"),
            PlanStatus::Approved => write!(f, "approved"),
            PlanStatus::Active => write!(f, "active"),
            PlanStatus::Completed => write!(f, "completed"),
            PlanStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl PlanStatus {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "draft" => Some(Self::Draft),
            "approved" => Some(Self::Approved),
            "active" => Some(Self::Active),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPackageStatus {
    Draft,
    Planned,
    Ready,
    InProgress,
    Review,
    Done,
    Cancelled,
}

impl std::fmt::Display for WorkPackageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkPackageStatus::Draft => write!(f, "draft"),
            WorkPackageStatus::Planned => write!(f, "planned"),
            WorkPackageStatus::Ready => write!(f, "ready"),
            WorkPackageStatus::InProgress => write!(f, "in_progress"),
            WorkPackageStatus::Review => write!(f, "review"),
            WorkPackageStatus::Done => write!(f, "done"),
            WorkPackageStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl WorkPackageStatus {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "draft" => Some(Self::Draft),
            "planned" => Some(Self::Planned),
            "ready" => Some(Self::Ready),
            "in_progress" => Some(Self::InProgress),
            "review" => Some(Self::Review),
            "done" => Some(Self::Done),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub intent: String,
    pub status: PlanStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub current_phase: MissionPhase,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<Assumption>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<Blocker>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_touched: Vec<String>,
    #[serde(default)]
    pub autonomy_level: AutonomyLevel,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkPackage {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<Uuid>,
    pub intent: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
    pub ordinal: i32,
    pub status: WorkPackageStatus,
    /// IDs of other WorkPackages that must be Done before this one becomes Ready.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Free-text progress note (updated via update_work_package_progress).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub validation_status: ValidationStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validation_evidence: Vec<ValidationEvidence>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub change_rationale: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Risk register
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskStatus {
    Identified,
    Acknowledged,
    Mitigated,
    Verified,
    Accepted,
}

impl std::fmt::Display for RiskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskStatus::Identified => write!(f, "identified"),
            RiskStatus::Acknowledged => write!(f, "acknowledged"),
            RiskStatus::Mitigated => write!(f, "mitigated"),
            RiskStatus::Verified => write!(f, "verified"),
            RiskStatus::Accepted => write!(f, "accepted"),
        }
    }
}

impl RiskStatus {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "identified" => Some(Self::Identified),
            "acknowledged" => Some(Self::Acknowledged),
            "mitigated" => Some(Self::Mitigated),
            "verified" => Some(Self::Verified),
            "accepted" => Some(Self::Accepted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskSeverity {
    High,
    Medium,
    Low,
}

impl std::fmt::Display for RiskSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskSeverity::High => write!(f, "high"),
            RiskSeverity::Medium => write!(f, "medium"),
            RiskSeverity::Low => write!(f, "low"),
        }
    }
}

impl RiskSeverity {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Risk {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Uuid>,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub category: String,
    pub severity: RiskSeverity,
    pub status: RiskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mitigation_plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_criteria: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Mission phase + autonomy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MissionPhase {
    #[default]
    Investigate,
    Plan,
    Clarify,
    Implement,
    Validate,
    Review,
    Finalize,
}

impl std::fmt::Display for MissionPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MissionPhase::Investigate => write!(f, "investigate"),
            MissionPhase::Plan => write!(f, "plan"),
            MissionPhase::Clarify => write!(f, "clarify"),
            MissionPhase::Implement => write!(f, "implement"),
            MissionPhase::Validate => write!(f, "validate"),
            MissionPhase::Review => write!(f, "review"),
            MissionPhase::Finalize => write!(f, "finalize"),
        }
    }
}

impl MissionPhase {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "investigate" => Some(Self::Investigate),
            "plan" => Some(Self::Plan),
            "clarify" => Some(Self::Clarify),
            "implement" => Some(Self::Implement),
            "validate" => Some(Self::Validate),
            "review" => Some(Self::Review),
            "finalize" => Some(Self::Finalize),
            _ => None,
        }
    }

    /// Returns the next phase in the default sequence, or None if at the end.
    pub fn next(&self) -> Option<Self> {
        match self {
            MissionPhase::Investigate => Some(MissionPhase::Plan),
            MissionPhase::Plan => Some(MissionPhase::Clarify),
            MissionPhase::Clarify => Some(MissionPhase::Implement),
            MissionPhase::Implement => Some(MissionPhase::Validate),
            MissionPhase::Validate => Some(MissionPhase::Review),
            MissionPhase::Review => Some(MissionPhase::Finalize),
            MissionPhase::Finalize => None,
        }
    }

    /// All phases as (label, index) for the timeline bar.
    pub fn all() -> &'static [&'static str] {
        &["investigate", "plan", "clarify", "implement", "validate", "review", "finalize"]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel {
    #[default]
    Supervised,
    Balanced,
    Autonomous,
}

impl std::fmt::Display for AutonomyLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutonomyLevel::Supervised => write!(f, "supervised"),
            AutonomyLevel::Balanced => write!(f, "balanced"),
            AutonomyLevel::Autonomous => write!(f, "autonomous"),
        }
    }
}

impl AutonomyLevel {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "supervised" => Some(Self::Supervised),
            "balanced" => Some(Self::Balanced),
            "autonomous" => Some(Self::Autonomous),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AssumptionStatus {
    #[default]
    Unverified,
    Confirmed,
    Rejected,
}

impl std::fmt::Display for AssumptionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssumptionStatus::Unverified => write!(f, "unverified"),
            AssumptionStatus::Confirmed => write!(f, "confirmed"),
            AssumptionStatus::Rejected => write!(f, "rejected"),
        }
    }
}

impl AssumptionStatus {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "unverified" => Some(Self::Unverified),
            "confirmed" => Some(Self::Confirmed),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BlockerStatus {
    #[default]
    Open,
    Resolved,
}

impl std::fmt::Display for BlockerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockerStatus::Open => write!(f, "open"),
            BlockerStatus::Resolved => write!(f, "resolved"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assumption {
    pub text: String,
    pub status: AssumptionStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_agent: String,
    pub created_at: DateTime<Utc>,
}

impl Assumption {
    pub fn new(text: &str, source_agent: &str) -> Self {
        Self {
            text: text.to_string(),
            status: AssumptionStatus::Unverified,
            source_agent: source_agent.to_string(),
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blocker {
    pub text: String,
    pub status: BlockerStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_agent: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
}

impl Blocker {
    pub fn new(text: &str, source_agent: &str) -> Self {
        Self {
            text: text.to_string(),
            status: BlockerStatus::Open,
            source_agent: source_agent.to_string(),
            created_at: Utc::now(),
            resolved_at: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Validation + change sets (Phase C)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ValidationStatus {
    #[default]
    Pending,
    Passing,
    Failing,
    Skipped,
}

impl std::fmt::Display for ValidationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationStatus::Pending => write!(f, "pending"),
            ValidationStatus::Passing => write!(f, "passing"),
            ValidationStatus::Failing => write!(f, "failing"),
            ValidationStatus::Skipped => write!(f, "skipped"),
        }
    }
}

impl ValidationStatus {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "passing" => Some(Self::Passing),
            "failing" => Some(Self::Failing),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationEvidence {
    pub evidence_type: String,
    pub content: String,
    pub passed: bool,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChangeType {
    Added,
    #[default]
    Modified,
    Deleted,
    Renamed,
}

impl std::fmt::Display for ChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeType::Added => write!(f, "added"),
            ChangeType::Modified => write!(f, "modified"),
            ChangeType::Deleted => write!(f, "deleted"),
            ChangeType::Renamed => write!(f, "renamed"),
        }
    }
}

impl ChangeType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "added" => Some(Self::Added),
            "modified" => Some(Self::Modified),
            "deleted" => Some(Self::Deleted),
            "renamed" => Some(Self::Renamed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSet {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wp_id: Option<Uuid>,
    pub file_path: String,
    pub change_type: ChangeType,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rationale: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diff_excerpt: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// File claims (advisory locking for multi-agent coordination)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileClaim {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub file_path: String,
    /// Denormalized agent name — no FK needed, stable identity across sessions.
    pub agent_name: String,
    pub claimed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Thread guardrails
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadGuardrails {
    pub id: Uuid,
    pub thread_id: Uuid,
    pub workspace_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_id: Option<Uuid>,
    pub locked: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget_usd: Option<f64>,
    #[serde(default)]
    pub cost_spent_usd: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailCheck {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
