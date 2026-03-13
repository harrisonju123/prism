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
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Dead,
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentState::Idle => write!(f, "idle"),
            AgentState::Working => write!(f, "working"),
            AgentState::Blocked => write!(f, "blocked"),
            AgentState::Dead => write!(f, "dead"),
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
    /// UUIDs of threads that must be archived before this one can start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<Uuid>,
    /// Agent-reported confidence [0.0, 1.0]. None = not yet assessed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Running cost charged to this thread in USD.
    #[serde(default)]
    pub cost_spent_usd: f64,
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
