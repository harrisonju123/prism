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

    // --- Snapshot (1) ---
    async fn create_snapshot(&self, workspace_id: Uuid, label: &str) -> Result<Snapshot>;

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

    // --- Guardrails (3) ---
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

    // --- Overview (1) ---
    async fn get_workspace_overview(&self, workspace_id: Uuid) -> Result<WorkspaceOverview>;

    // --- Inbox (5) ---
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
    async fn list_inbox_entries(
        &self,
        workspace_id: Uuid,
        filters: InboxFilters,
    ) -> Result<Vec<InboxEntry>>;
    async fn mark_inbox_read(&self, workspace_id: Uuid, id: Uuid) -> Result<()>;
    async fn dismiss_inbox_entry(&self, workspace_id: Uuid, id: Uuid) -> Result<()>;

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

    // --- Messages (4) ---
    async fn send_message(
        &self,
        workspace_id: Uuid,
        from_agent: &str,
        to_agent: &str,
        content: &str,
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
}
