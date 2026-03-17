pub mod sqlite;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::Result;
use crate::model::*;

#[derive(Debug, Default)]
pub struct MemoryFilters {
    pub thread_id: Option<Uuid>,
    pub thread_name: Option<String>,
    pub tags: Option<Vec<String>>,
    pub global_only: bool,
}

#[derive(Debug, Default)]
pub struct ActivityFilters {
    pub since: Option<DateTime<Utc>>,
    pub actor: Option<String>,
    pub thread_id: Option<Uuid>,
    pub limit: i64,
}

#[derive(Debug)]
pub struct InboxFilters {
    /// Only return entries where read = false.
    pub unread_only: bool,
    /// Filter to a specific entry type.
    pub entry_type: Option<InboxEntryType>,
    /// When false (default) exclude dismissed entries.
    /// When true include dismissed entries too.
    pub include_dismissed: bool,
    /// Cap result set. Defaults to 200.
    pub limit: i64,
}

impl Default for InboxFilters {
    fn default() -> Self {
        Self {
            unread_only: false,
            entry_type: None,
            include_dismissed: false,
            limit: 200,
        }
    }
}

#[async_trait]
pub trait Store: Send + Sync {
    // --- Workspace (2) ---
    async fn init_workspace(&self, name: &str, desc: &str) -> Result<Workspace>;
    async fn get_workspace(&self, id: Uuid) -> Result<Workspace>;

    // --- Thread (4) ---
    async fn create_thread(
        &self,
        workspace_id: Uuid,
        name: &str,
        desc: &str,
        tags: Vec<String>,
    ) -> Result<Thread>;
    async fn get_thread(&self, workspace_id: Uuid, name: &str) -> Result<Thread>;
    async fn list_threads(
        &self,
        workspace_id: Uuid,
        status: Option<ThreadStatus>,
    ) -> Result<Vec<Thread>>;
    async fn archive_thread(&self, workspace_id: Uuid, name: &str) -> Result<Thread>;

    // --- Memory (3) ---
    async fn save_memory(
        &self,
        workspace_id: Uuid,
        key: &str,
        value: &str,
        thread_id: Option<Uuid>,
        source: &str,
        tags: Vec<String>,
    ) -> Result<Memory>;
    async fn load_memories(
        &self,
        workspace_id: Uuid,
        filters: MemoryFilters,
    ) -> Result<Vec<Memory>>;
    async fn delete_memory(&self, workspace_id: Uuid, key: &str) -> Result<()>;

    // --- Decision (5) ---
    async fn save_decision(
        &self,
        workspace_id: Uuid,
        title: &str,
        content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
        scope: DecisionScope,
    ) -> Result<Decision>;
    async fn list_decisions(
        &self,
        workspace_id: Uuid,
        thread_id: Option<Uuid>,
        tags: Option<Vec<String>>,
    ) -> Result<Vec<Decision>>;
    async fn supersede_decision(
        &self,
        workspace_id: Uuid,
        old_id: Uuid,
        new_title: &str,
        new_content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
    ) -> Result<Decision>;
    async fn revoke_decision(&self, workspace_id: Uuid, id: Uuid) -> Result<Decision>;
    async fn pending_decision_notifications(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
    ) -> Result<Vec<Decision>>;
    async fn acknowledge_decisions(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        decision_ids: Vec<Uuid>,
    ) -> Result<()>;

    // --- Agent (6) ---
    async fn checkin(
        &self,
        workspace_id: Uuid,
        name: &str,
        capabilities: Vec<String>,
        thread_id: Option<Uuid>,
        branch: Option<String>,
        worktree_path: Option<String>,
    ) -> Result<CheckinContext>;
    async fn checkout(
        &self,
        workspace_id: Uuid,
        name: &str,
        summary: &str,
        findings: Vec<String>,
        files_touched: Vec<String>,
        next_steps: Vec<String>,
    ) -> Result<AgentSession>;
    async fn list_agents(&self, workspace_id: Uuid) -> Result<Vec<AgentStatus>>;
    async fn heartbeat(&self, workspace_id: Uuid, name: &str) -> Result<()>;
    async fn set_agent_state(
        &self,
        workspace_id: Uuid,
        name: &str,
        state: AgentState,
    ) -> Result<()>;
    async fn reap_dead_agents(&self, workspace_id: Uuid, timeout_secs: i64) -> Result<Vec<String>>;
    /// Persist mid-session state without closing the session (crash recovery).
    async fn update_session(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        summary: &str,
        files_touched: Vec<String>,
    ) -> Result<()>;

    // --- Context (2) ---
    async fn recall_thread(&self, workspace_id: Uuid, thread_name: &str) -> Result<ThreadContext>;
    async fn recall_by_tags(
        &self,
        workspace_id: Uuid,
        tags: Vec<String>,
        since: Option<DateTime<Utc>>,
    ) -> Result<RecallResult>;

    // --- Activity (1) ---
    async fn list_activity(
        &self,
        workspace_id: Uuid,
        filters: ActivityFilters,
    ) -> Result<Vec<ActivityEntry>>;

    // --- Snapshot (2) ---
    async fn create_snapshot(&self, workspace_id: Uuid, label: &str) -> Result<Snapshot>;
    async fn list_snapshots(&self, workspace_id: Uuid, limit: Option<i64>) -> Result<Vec<Snapshot>>;

    // --- Handoff (4) ---
    async fn create_handoff(
        &self,
        workspace_id: Uuid,
        from_agent: &str,
        task: &str,
        thread_id: Option<Uuid>,
        constraints: HandoffConstraints,
        mode: HandoffMode,
    ) -> Result<Handoff>;
    async fn accept_handoff(
        &self,
        workspace_id: Uuid,
        handoff_id: Uuid,
        agent_name: &str,
    ) -> Result<Handoff>;
    async fn complete_handoff(
        &self,
        workspace_id: Uuid,
        handoff_id: Uuid,
        result: serde_json::Value,
    ) -> Result<Handoff>;
    async fn list_handoffs(
        &self,
        workspace_id: Uuid,
        agent_name: Option<&str>,
        status: Option<HandoffStatus>,
    ) -> Result<Vec<Handoff>>;
    async fn start_handoff(&self, workspace_id: Uuid, handoff_id: Uuid) -> Result<Handoff>;
    async fn fail_handoff(
        &self,
        workspace_id: Uuid,
        handoff_id: Uuid,
        reason: &str,
    ) -> Result<Handoff>;
    async fn cancel_handoff(&self, workspace_id: Uuid, handoff_id: Uuid) -> Result<Handoff>;
    async fn check_handoff_constraints(
        &self,
        workspace_id: Uuid,
        handoff_id: Uuid,
        tool_name: &str,
        file_path: Option<&str>,
    ) -> Result<()>;

    // --- Guardrails (4) ---
    async fn set_guardrails(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
        guardrails: ThreadGuardrails,
    ) -> Result<ThreadGuardrails>;
    async fn get_guardrails(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
    ) -> Result<Option<ThreadGuardrails>>;
    async fn check_guardrail(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
        agent_name: &str,
        tool_name: &str,
        file_path: Option<&str>,
    ) -> Result<GuardrailCheck>;
    async fn increment_guardrail_cost(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
        amount_usd: f64,
    ) -> Result<()>;

    // --- Overview (1) ---
    async fn get_workspace_overview(&self, workspace_id: Uuid) -> Result<WorkspaceOverview>;

    // --- Inbox (7) ---
    async fn create_inbox_entry(
        &self,
        workspace_id: Uuid,
        entry_type: InboxEntryType,
        title: &str,
        body: &str,
        severity: InboxSeverity,
        source_agent: Option<&str>,
        ref_type: Option<&str>,
        ref_id: Option<Uuid>,
    ) -> Result<InboxEntry>;
    /// Insert a new inbox entry or update the body of a recent matching entry (dedup).
    /// "Similar" = same (entry_type, source_agent) + matching title prefix + not dismissed/resolved
    /// + within cooldown_secs (default 300).
    async fn create_or_update_inbox_entry(
        &self,
        workspace_id: Uuid,
        entry_type: InboxEntryType,
        title: &str,
        body: &str,
        severity: InboxSeverity,
        source_agent: Option<&str>,
        ref_type: Option<&str>,
        ref_id: Option<Uuid>,
        cooldown_secs: Option<u64>,
    ) -> Result<InboxEntry>;
    async fn list_inbox_entries(
        &self,
        workspace_id: Uuid,
        filters: InboxFilters,
    ) -> Result<Vec<InboxEntry>>;
    async fn mark_inbox_read(&self, workspace_id: Uuid, id: Uuid) -> Result<()>;
    async fn dismiss_inbox_entry(&self, workspace_id: Uuid, id: Uuid) -> Result<()>;
    async fn get_inbox_entry(&self, workspace_id: Uuid, id: Uuid) -> Result<InboxEntry>;
    async fn resolve_inbox_entry(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        resolution: &str,
    ) -> Result<InboxEntry>;
    /// Auto-dismiss unread Completed (or other) entries older than max_age_secs.
    /// Returns the count of dismissed entries.
    async fn dismiss_expired_entries(
        &self,
        workspace_id: Uuid,
        entry_type: InboxEntryType,
        max_age_secs: u64,
    ) -> Result<u64>;

    // --- Plan (4) ---
    async fn create_plan(&self, workspace_id: Uuid, intent: &str) -> Result<Plan>;
    async fn get_plan(&self, workspace_id: Uuid, plan_id: Uuid) -> Result<Plan>;
    async fn update_plan_status(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        status: PlanStatus,
    ) -> Result<Plan>;
    async fn list_plans(&self, workspace_id: Uuid, status: Option<PlanStatus>)
    -> Result<Vec<Plan>>;

    // --- Plan mission metadata ---

    async fn update_plan_phase(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        phase: MissionPhase,
    ) -> Result<Plan>;

    async fn update_plan_metadata(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        description: Option<&str>,
        constraints: Option<Vec<String>>,
        autonomy: Option<AutonomyLevel>,
    ) -> Result<Plan>;

    async fn add_plan_assumption(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        text: &str,
        source_agent: &str,
    ) -> Result<Plan>;

    async fn update_plan_assumption(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        index: usize,
        status: AssumptionStatus,
    ) -> Result<Plan>;

    async fn add_plan_blocker(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        text: &str,
        source_agent: &str,
    ) -> Result<Plan>;

    async fn resolve_plan_blocker(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        index: usize,
    ) -> Result<Plan>;

    async fn record_plan_file_touched(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        path: &str,
    ) -> Result<Plan>;

    /// Returns the most recently updated active or approved plan.
    async fn get_active_plan(&self, workspace_id: Uuid) -> Result<Option<Plan>>;

    // --- Change sets ---

    async fn record_change_set(
        &self,
        workspace_id: Uuid,
        plan_id: Option<Uuid>,
        wp_id: Option<Uuid>,
        file_path: &str,
        change_type: ChangeType,
        rationale: &str,
        diff_excerpt: &str,
    ) -> Result<ChangeSet>;

    async fn list_change_sets(
        &self,
        workspace_id: Uuid,
        plan_id: Option<Uuid>,
        wp_id: Option<Uuid>,
    ) -> Result<Vec<ChangeSet>>;

    // --- WorkPackage validation ---

    async fn record_validation_evidence(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        evidence: ValidationEvidence,
    ) -> Result<WorkPackage>;

    async fn update_validation_status(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        status: ValidationStatus,
    ) -> Result<WorkPackage>;

    // --- WorkPackage (6) ---
    async fn create_work_package(
        &self,
        workspace_id: Uuid,
        plan_id: Option<Uuid>,
        intent: &str,
        acceptance_criteria: Vec<String>,
        ordinal: i32,
        depends_on: Vec<Uuid>,
        tags: Vec<String>,
    ) -> Result<WorkPackage>;
    async fn get_work_package(&self, workspace_id: Uuid, wp_id: Uuid) -> Result<WorkPackage>;
    async fn update_work_package_status(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        status: WorkPackageStatus,
    ) -> Result<WorkPackage>;
    async fn assign_work_package(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        agent_name: &str,
        thread_id: Uuid,
    ) -> Result<WorkPackage>;
    async fn list_work_packages(
        &self,
        workspace_id: Uuid,
        plan_id: Option<Uuid>,
        status: Option<WorkPackageStatus>,
    ) -> Result<Vec<WorkPackage>>;
    /// For all Planned WPs in a plan, flip to Ready if all depends_on WPs are Done.
    async fn refresh_work_package_readiness(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
    ) -> Result<Vec<WorkPackage>>;
    async fn update_work_package_progress(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        status: WorkPackageStatus,
        progress_note: &str,
    ) -> Result<WorkPackage>;

    // --- Messages (4) ---
    async fn send_message(
        &self,
        workspace_id: Uuid,
        from_agent: &str,
        to_agent: &str,
        content: &str,
        conversation_id: Option<Uuid>,
    ) -> Result<Message>;
    async fn list_messages(
        &self,
        workspace_id: Uuid,
        to_agent: &str,
        unread_only: bool,
    ) -> Result<Vec<Message>>;
    async fn mark_messages_read(&self, workspace_id: Uuid, to_agent: &str) -> Result<()>;
    async fn count_unread_messages(&self, workspace_id: Uuid, to_agent: &str) -> Result<i64>;
    async fn count_all_unread_messages(
        &self,
        workspace_id: Uuid,
    ) -> Result<std::collections::HashMap<String, i64>>;
    async fn prune_old_messages(&self, workspace_id: Uuid) -> Result<()>;

    // --- Risk (5) ---
    async fn create_risk(
        &self,
        workspace_id: Uuid,
        thread_id: Option<Uuid>,
        title: &str,
        description: &str,
        category: &str,
        severity: RiskSeverity,
        source_agent: Option<&str>,
        tags: Vec<String>,
    ) -> Result<Risk>;
    async fn update_risk_status(
        &self,
        workspace_id: Uuid,
        risk_id: Uuid,
        status: RiskStatus,
        mitigation_plan: Option<&str>,
        verification_criteria: Option<&str>,
    ) -> Result<Risk>;
    async fn get_risk(&self, workspace_id: Uuid, risk_id: Uuid) -> Result<Risk>;
    async fn list_risks(
        &self,
        workspace_id: Uuid,
        status: Option<RiskStatus>,
        thread_id: Option<Uuid>,
    ) -> Result<Vec<Risk>>;
    async fn list_unverified_risks(
        &self,
        workspace_id: Uuid,
        agent_name: Option<&str>,
    ) -> Result<Vec<Risk>>;

    // --- FileClaim (4) ---
    async fn claim_file(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        file_path: &str,
        ttl_secs: Option<i64>,
    ) -> Result<FileClaim>;
    async fn release_file(
        &self,
        workspace_id: Uuid,
        file_path: &str,
        agent_name: &str,
    ) -> Result<()>;
    async fn check_file_claim(
        &self,
        workspace_id: Uuid,
        file_path: &str,
    ) -> Result<Option<FileClaim>>;
    async fn list_file_claims(
        &self,
        workspace_id: Uuid,
        agent_name: Option<&str>,
    ) -> Result<Vec<FileClaim>>;
    async fn release_all_claims_for_agent(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
    ) -> Result<u64>;
}
