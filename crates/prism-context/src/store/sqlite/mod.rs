#[macro_use]
pub mod types;
mod activity;
mod agent;
mod context;
mod decision;
mod file_claim;
mod guardrail;
mod handoff;
mod inbox;
mod memory;
mod message;
mod migrate;
mod plan;
mod snapshot;
#[cfg(test)]
mod tests;
mod thread;
mod work_package;
mod workspace;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

use crate::error::Result;
use crate::model::*;
use crate::store::{ActivityFilters, InboxFilters, MemoryFilters, Store};

pub struct SqliteStore {
    pub(crate) pool: SqlitePool,
}

#[cfg(test)]
impl SqliteStore {
    pub(crate) async fn open_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .map_err(|e| crate::error::Error::Internal(format!("open memory sqlite: {e}")))?;

        migrate::run_migrations(&pool).await?;

        Ok(Self { pool })
    }
}

impl SqliteStore {
    pub async fn open(path: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .map_err(|e| crate::error::Error::Internal(format!("open sqlite: {e}")))?;

        migrate::run_migrations(&pool).await?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn init_workspace(&self, name: &str, desc: &str) -> Result<Workspace> {
        self.init_workspace_impl(name, desc).await
    }

    async fn get_workspace(&self, id: Uuid) -> Result<Workspace> {
        self.get_workspace_impl(id).await
    }

    async fn create_thread(
        &self,
        workspace_id: Uuid,
        name: &str,
        desc: &str,
        tags: Vec<String>,
    ) -> Result<Thread> {
        self.create_thread_impl(workspace_id, name, desc, tags)
            .await
    }

    async fn get_thread(&self, workspace_id: Uuid, name: &str) -> Result<Thread> {
        self.get_thread_impl(workspace_id, name).await
    }

    async fn list_threads(
        &self,
        workspace_id: Uuid,
        status: Option<ThreadStatus>,
    ) -> Result<Vec<Thread>> {
        self.list_threads_impl(workspace_id, status).await
    }

    async fn archive_thread(&self, workspace_id: Uuid, name: &str) -> Result<Thread> {
        self.archive_thread_impl(workspace_id, name).await
    }

    async fn save_memory(
        &self,
        workspace_id: Uuid,
        key: &str,
        value: &str,
        thread_id: Option<Uuid>,
        source: &str,
        tags: Vec<String>,
    ) -> Result<Memory> {
        self.save_memory_impl(workspace_id, key, value, thread_id, source, tags)
            .await
    }

    async fn load_memories(
        &self,
        workspace_id: Uuid,
        filters: MemoryFilters,
    ) -> Result<Vec<Memory>> {
        self.load_memories_impl(workspace_id, filters).await
    }

    async fn delete_memory(&self, workspace_id: Uuid, key: &str) -> Result<()> {
        self.delete_memory_impl(workspace_id, key).await
    }

    async fn save_decision(
        &self,
        workspace_id: Uuid,
        title: &str,
        content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
        scope: DecisionScope,
    ) -> Result<Decision> {
        self.save_decision_impl(workspace_id, title, content, thread_id, tags, scope)
            .await
    }

    async fn list_decisions(
        &self,
        workspace_id: Uuid,
        thread_id: Option<Uuid>,
        tags: Option<Vec<String>>,
    ) -> Result<Vec<Decision>> {
        self.list_decisions_impl(workspace_id, thread_id, tags)
            .await
    }

    async fn supersede_decision(
        &self,
        workspace_id: Uuid,
        old_id: Uuid,
        new_title: &str,
        new_content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
    ) -> Result<Decision> {
        self.supersede_decision_impl(
            workspace_id,
            old_id,
            new_title,
            new_content,
            thread_id,
            tags,
        )
        .await
    }

    async fn revoke_decision(&self, workspace_id: Uuid, id: Uuid) -> Result<Decision> {
        self.revoke_decision_impl(workspace_id, id).await
    }

    async fn pending_decision_notifications(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
    ) -> Result<Vec<Decision>> {
        self.pending_decision_notifications_impl(workspace_id, agent_name)
            .await
    }

    async fn acknowledge_decisions(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        decision_ids: Vec<Uuid>,
    ) -> Result<()> {
        self.acknowledge_decisions_impl(workspace_id, agent_name, decision_ids)
            .await
    }

    async fn checkin(
        &self,
        workspace_id: Uuid,
        name: &str,
        capabilities: Vec<String>,
        thread_id: Option<Uuid>,
    ) -> Result<CheckinContext> {
        self.checkin_impl(workspace_id, name, capabilities, thread_id)
            .await
    }

    async fn checkout(
        &self,
        workspace_id: Uuid,
        name: &str,
        summary: &str,
        findings: Vec<String>,
        files_touched: Vec<String>,
        next_steps: Vec<String>,
    ) -> Result<AgentSession> {
        self.checkout_impl(
            workspace_id,
            name,
            summary,
            findings,
            files_touched,
            next_steps,
        )
        .await
    }

    async fn list_agents(&self, workspace_id: Uuid) -> Result<Vec<AgentStatus>> {
        self.list_agents_impl(workspace_id).await
    }

    async fn heartbeat(&self, workspace_id: Uuid, name: &str) -> Result<()> {
        self.heartbeat_impl(workspace_id, name).await
    }

    async fn set_agent_state(
        &self,
        workspace_id: Uuid,
        name: &str,
        state: AgentState,
    ) -> Result<()> {
        self.set_agent_state_impl(workspace_id, name, state).await
    }

    async fn reap_dead_agents(&self, workspace_id: Uuid, timeout_secs: i64) -> Result<Vec<String>> {
        self.reap_dead_agents_impl(workspace_id, timeout_secs).await
    }

    async fn update_session(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        summary: &str,
        files_touched: Vec<String>,
    ) -> Result<()> {
        self.update_session_impl(workspace_id, agent_name, summary, files_touched)
            .await
    }

    async fn recall_thread(&self, workspace_id: Uuid, thread_name: &str) -> Result<ThreadContext> {
        self.recall_thread_impl(workspace_id, thread_name).await
    }

    async fn recall_by_tags(
        &self,
        workspace_id: Uuid,
        tags: Vec<String>,
        since: Option<DateTime<Utc>>,
    ) -> Result<RecallResult> {
        self.recall_by_tags_impl(workspace_id, tags, since).await
    }

    async fn list_activity(
        &self,
        workspace_id: Uuid,
        filters: ActivityFilters,
    ) -> Result<Vec<ActivityEntry>> {
        self.list_activity_impl(workspace_id, filters).await
    }

    async fn create_snapshot(&self, workspace_id: Uuid, label: &str) -> Result<Snapshot> {
        self.create_snapshot_impl(workspace_id, label).await
    }

    async fn create_handoff(
        &self,
        workspace_id: Uuid,
        from_agent: &str,
        task: &str,
        thread_id: Option<Uuid>,
        constraints: HandoffConstraints,
        mode: HandoffMode,
    ) -> Result<Handoff> {
        self.create_handoff_impl(workspace_id, from_agent, task, thread_id, constraints, mode)
            .await
    }

    async fn accept_handoff(
        &self,
        workspace_id: Uuid,
        handoff_id: Uuid,
        agent_name: &str,
    ) -> Result<Handoff> {
        self.accept_handoff_impl(workspace_id, handoff_id, agent_name)
            .await
    }

    async fn complete_handoff(
        &self,
        workspace_id: Uuid,
        handoff_id: Uuid,
        result: serde_json::Value,
    ) -> Result<Handoff> {
        self.complete_handoff_impl(workspace_id, handoff_id, result)
            .await
    }

    async fn list_handoffs(
        &self,
        workspace_id: Uuid,
        agent_name: Option<&str>,
        status: Option<HandoffStatus>,
    ) -> Result<Vec<Handoff>> {
        self.list_handoffs_impl(workspace_id, agent_name, status)
            .await
    }

    async fn set_guardrails(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
        guardrails: ThreadGuardrails,
    ) -> Result<ThreadGuardrails> {
        self.set_guardrails_impl(workspace_id, thread_name, guardrails)
            .await
    }

    async fn get_guardrails(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
    ) -> Result<Option<ThreadGuardrails>> {
        self.get_guardrails_impl(workspace_id, thread_name).await
    }

    async fn check_guardrail(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
        agent_name: &str,
        tool_name: &str,
        file_path: Option<&str>,
    ) -> Result<GuardrailCheck> {
        self.check_guardrail_impl(workspace_id, thread_name, agent_name, tool_name, file_path)
            .await
    }

    async fn increment_guardrail_cost(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
        amount_usd: f64,
    ) -> Result<()> {
        self.increment_guardrail_cost_impl(workspace_id, thread_name, amount_usd)
            .await
    }

    async fn get_workspace_overview(&self, workspace_id: Uuid) -> Result<WorkspaceOverview> {
        self.get_workspace_overview_impl(workspace_id).await
    }

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
    ) -> Result<InboxEntry> {
        self.create_inbox_entry_impl(
            workspace_id,
            entry_type,
            title,
            body,
            severity,
            source_agent,
            ref_type,
            ref_id,
        )
        .await
    }

    async fn list_inbox_entries(
        &self,
        workspace_id: Uuid,
        filters: InboxFilters,
    ) -> Result<Vec<InboxEntry>> {
        self.list_inbox_entries_impl(workspace_id, filters).await
    }

    async fn mark_inbox_read(&self, workspace_id: Uuid, id: Uuid) -> Result<()> {
        self.mark_inbox_read_impl(workspace_id, id).await
    }

    async fn dismiss_inbox_entry(&self, workspace_id: Uuid, id: Uuid) -> Result<()> {
        self.dismiss_inbox_entry_impl(workspace_id, id).await
    }

    async fn get_inbox_entry(&self, workspace_id: Uuid, id: Uuid) -> Result<InboxEntry> {
        self.get_inbox_entry_impl(workspace_id, id).await
    }

    async fn resolve_inbox_entry(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        resolution: &str,
    ) -> Result<InboxEntry> {
        self.resolve_inbox_entry_impl(workspace_id, id, resolution).await
    }

    async fn send_message(
        &self,
        workspace_id: Uuid,
        from_agent: &str,
        to_agent: &str,
        content: &str,
        conversation_id: Option<Uuid>,
    ) -> Result<Message> {
        self.send_message_impl(workspace_id, from_agent, to_agent, content, conversation_id)
            .await
    }

    async fn list_messages(
        &self,
        workspace_id: Uuid,
        to_agent: &str,
        unread_only: bool,
    ) -> Result<Vec<Message>> {
        self.list_messages_impl(workspace_id, to_agent, unread_only)
            .await
    }

    async fn mark_messages_read(&self, workspace_id: Uuid, to_agent: &str) -> Result<()> {
        self.mark_messages_read_impl(workspace_id, to_agent).await
    }

    async fn count_unread_messages(&self, workspace_id: Uuid, to_agent: &str) -> Result<i64> {
        self.count_unread_messages_impl(workspace_id, to_agent)
            .await
    }

    async fn count_all_unread_messages(
        &self,
        workspace_id: Uuid,
    ) -> Result<std::collections::HashMap<String, i64>> {
        self.count_all_unread_messages_impl(workspace_id).await
    }

    async fn prune_old_messages(&self, workspace_id: Uuid) -> Result<()> {
        self.prune_old_messages_impl(workspace_id).await
    }

    async fn create_plan(&self, workspace_id: Uuid, intent: &str) -> Result<Plan> {
        self.create_plan_impl(workspace_id, intent).await
    }

    async fn get_plan(&self, workspace_id: Uuid, plan_id: Uuid) -> Result<Plan> {
        self.get_plan_impl(workspace_id, plan_id).await
    }

    async fn update_plan_status(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
        status: PlanStatus,
    ) -> Result<Plan> {
        self.update_plan_status_impl(workspace_id, plan_id, status)
            .await
    }

    async fn list_plans(
        &self,
        workspace_id: Uuid,
        status: Option<PlanStatus>,
    ) -> Result<Vec<Plan>> {
        self.list_plans_impl(workspace_id, status).await
    }

    async fn create_work_package(
        &self,
        workspace_id: Uuid,
        plan_id: Option<Uuid>,
        intent: &str,
        acceptance_criteria: Vec<String>,
        ordinal: i32,
        depends_on: Vec<Uuid>,
        tags: Vec<String>,
    ) -> Result<WorkPackage> {
        self.create_work_package_impl(
            workspace_id,
            plan_id,
            intent,
            acceptance_criteria,
            ordinal,
            depends_on,
            tags,
        )
        .await
    }

    async fn get_work_package(&self, workspace_id: Uuid, wp_id: Uuid) -> Result<WorkPackage> {
        self.get_work_package_impl(workspace_id, wp_id).await
    }

    async fn update_work_package_status(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        status: WorkPackageStatus,
    ) -> Result<WorkPackage> {
        self.update_work_package_status_impl(workspace_id, wp_id, status)
            .await
    }

    async fn assign_work_package(
        &self,
        workspace_id: Uuid,
        wp_id: Uuid,
        agent_name: &str,
        thread_id: Uuid,
    ) -> Result<WorkPackage> {
        self.assign_work_package_impl(workspace_id, wp_id, agent_name, thread_id)
            .await
    }

    async fn list_work_packages(
        &self,
        workspace_id: Uuid,
        plan_id: Option<Uuid>,
        status: Option<WorkPackageStatus>,
    ) -> Result<Vec<WorkPackage>> {
        self.list_work_packages_impl(workspace_id, plan_id, status)
            .await
    }

    async fn refresh_work_package_readiness(
        &self,
        workspace_id: Uuid,
        plan_id: Uuid,
    ) -> Result<Vec<WorkPackage>> {
        self.refresh_work_package_readiness_impl(workspace_id, plan_id)
            .await
    }

    async fn claim_file(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        file_path: &str,
        ttl_secs: Option<i64>,
    ) -> Result<FileClaim> {
        self.claim_file_impl(workspace_id, agent_name, file_path, ttl_secs)
            .await
    }

    async fn release_file(
        &self,
        workspace_id: Uuid,
        file_path: &str,
        agent_name: &str,
    ) -> Result<()> {
        self.release_file_impl(workspace_id, file_path, agent_name)
            .await
    }

    async fn check_file_claim(
        &self,
        workspace_id: Uuid,
        file_path: &str,
    ) -> Result<Option<FileClaim>> {
        self.check_file_claim_impl(workspace_id, file_path).await
    }

    async fn list_file_claims(
        &self,
        workspace_id: Uuid,
        agent_name: Option<&str>,
    ) -> Result<Vec<FileClaim>> {
        self.list_file_claims_impl(workspace_id, agent_name).await
    }
}
