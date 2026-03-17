use std::sync::Arc;

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use gpui::{App, Global, WeakEntity};
use prism_context::model::{
    ActivityEntry, AgentSession, AgentState, AgentStatus, AssumptionStatus, AutonomyLevel,
    ChangeSet, CheckinContext, Decision, DecisionScope, FileClaim, GuardrailCheck, Handoff,
    HandoffConstraints, HandoffMode, HandoffStatus, InboxEntry, InboxEntryType, InboxSeverity,
    Memory, MissionPhase, Plan, PlanStatus, RecallResult, Risk, RiskSeverity, RiskStatus, Snapshot,
    Thread, ThreadContext, ThreadGuardrails, ThreadStatus, WorkPackage, WorkPackageStatus,
    WorkspaceOverview,
};
use prism_context::store::sqlite::SqliteStore;
use prism_context::store::{ActivityFilters, InboxFilters, MemoryFilters, Store};
use uuid::Uuid;

/// GPUI Global that holds prism context state. Lives on the main thread.
/// Use `handle()` to get a cloneable, Send-able handle for background work.
pub struct ContextService {
    inner: Option<ContextHandle>,
}

impl Global for ContextService {}

/// Thread-safe, cloneable handle to prism context. Can be sent to background threads.
#[derive(Clone)]
pub struct ContextHandle {
    store: Arc<SqliteStore>,
    workspace_id: Uuid,
    handle: tokio::runtime::Handle,
}

impl ContextService {
    /// Initialize the service by discovering `.prism/context.json` from `workspace_root`.
    /// Config discovery is synchronous; store opening is async (non-blocking).
    pub fn init(workspace_root: &std::path::Path, cx: &mut App) -> Result<()> {
        let tokio_handle = gpui_tokio::Tokio::handle(cx);

        let config_path = match prism_context::config::find_config(workspace_root) {
            Some(path) => path,
            None => {
                // Only auto-init inside git repos to avoid false positives.
                if !workspace_root.join(".git").exists() {
                    anyhow::bail!("prism-context not initialized in this workspace");
                }
                let workspace_name = workspace_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
                    .to_string();
                let dir = workspace_root.to_path_buf();
                let (path, _) = tokio_handle
                    .block_on(prism_context::config::auto_init(&dir, &workspace_name))
                    .map_err(|e| anyhow::anyhow!("prism-context auto-init: {e}"))?;
                add_context_to_gitignore(workspace_root);
                log::info!("prism-context auto-initialized at {:?}", path);
                path
            }
        };

        let config =
            prism_context::config::load_config(&config_path).map_err(|e| anyhow::anyhow!("{e}"))?;
        let workspace_id: Uuid = config
            .workspace_id
            .parse()
            .context("invalid workspace_id in .prism/context.json")?;

        let db_path = prism_context::config::resolve_db_path(&config_path, &config)
            .to_string_lossy()
            .to_string();

        // Set global immediately with no handle — callers already handle None gracefully
        cx.set_global(ContextService { inner: None });

        // Open the store on the tokio runtime, update global when ready
        let open_task = gpui_tokio::Tokio::spawn_result(cx, async move {
            use prism_context::store::Store as _;
            let store = SqliteStore::open(&db_path)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            // Prune stale messages on startup; non-fatal if it fails.
            let _ = store.prune_old_messages(workspace_id).await;
            anyhow::Ok(store)
        });

        cx.spawn(async move |cx| match open_task.await {
            Ok(store) => {
                cx.update(|cx| {
                    cx.global_mut::<ContextService>().inner = Some(ContextHandle {
                        store: Arc::new(store),
                        workspace_id,
                        handle: tokio_handle,
                    });
                });
            }
            Err(e) => {
                log::warn!("failed to open prism-context store: {e}");
            }
        })
        .detach();

        Ok(())
    }

    /// Get a cloneable handle that can be sent to background threads.
    pub fn handle(&self) -> Option<ContextHandle> {
        self.inner.clone()
    }
}

impl ContextHandle {
    pub fn workspace_id(&self) -> Uuid {
        self.workspace_id
    }

    fn run<F, T>(&self, f: F) -> Result<T>
    where
        F: std::future::Future<Output = std::result::Result<T, prism_context::error::Error>>,
    {
        self.handle.block_on(f).map_err(Into::into)
    }

    pub fn get_workspace_overview(&self) -> Result<WorkspaceOverview> {
        self.run(self.store.get_workspace_overview(self.workspace_id))
    }

    pub fn list_activity(&self, filters: ActivityFilters) -> Result<Vec<ActivityEntry>> {
        self.run(self.store.list_activity(self.workspace_id, filters))
    }

    pub fn list_agents(&self) -> Result<Vec<AgentStatus>> {
        self.run(self.store.list_agents(self.workspace_id))
    }

    pub fn checkin(
        &self,
        name: &str,
        capabilities: Vec<String>,
        thread_id: Option<Uuid>,
    ) -> Result<CheckinContext> {
        let name = name.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .checkin(wid, &name, capabilities, thread_id, None, None)
                .await
                .map_err(Into::into)
        })
    }

    pub fn checkout(
        &self,
        name: &str,
        summary: &str,
        findings: Vec<String>,
        files_touched: Vec<String>,
        next_steps: Vec<String>,
    ) -> Result<AgentSession> {
        let name = name.to_string();
        let summary = summary.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .checkout(wid, &name, &summary, findings, files_touched, next_steps)
                .await
                .map_err(Into::into)
        })
    }

    pub fn save_memory(
        &self,
        key: &str,
        value: &str,
        thread_name: Option<&str>,
        tags: Vec<String>,
    ) -> Result<Memory> {
        let key = key.to_string();
        let value = value.to_string();
        let source = "zed-panel".to_string();
        let thread_name = thread_name.map(|s| s.to_string());
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            let thread_id = if let Some(ref tn) = thread_name {
                store.get_thread(wid, tn).await.ok().map(|t| t.id)
            } else {
                None
            };
            store
                .save_memory(wid, &key, &value, thread_id, &source, tags)
                .await
                .map_err(Into::into)
        })
    }

    pub fn create_thread(
        &self,
        name: &str,
        description: &str,
        tags: Vec<String>,
    ) -> Result<Thread> {
        let name = name.to_string();
        let description = description.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .create_thread(wid, &name, &description, tags)
                .await
                .map_err(Into::into)
        })
    }

    pub fn get_thread(&self, name: &str) -> Result<Thread> {
        self.run(self.store.get_thread(self.workspace_id, name))
    }

    pub fn list_threads(&self, status: Option<ThreadStatus>) -> Result<Vec<Thread>> {
        self.run(self.store.list_threads(self.workspace_id, status))
    }

    pub fn archive_thread(&self, name: &str) -> Result<Thread> {
        self.run(self.store.archive_thread(self.workspace_id, name))
    }

    pub fn set_agent_state(&self, name: &str, state: AgentState) -> Result<()> {
        self.run(self.store.set_agent_state(self.workspace_id, name, state))
    }

    pub fn recall_thread(&self, thread_name: &str) -> Result<ThreadContext> {
        self.run(self.store.recall_thread(self.workspace_id, thread_name))
    }

    pub fn save_decision(
        &self,
        title: &str,
        content: &str,
        thread_id: Option<uuid::Uuid>,
        tags: Vec<String>,
        scope: DecisionScope,
    ) -> Result<Decision> {
        let title = title.to_string();
        let content = content.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .save_decision(wid, &title, &content, thread_id, tags, scope)
                .await
                .map_err(Into::into)
        })
    }

    pub fn create_handoff(
        &self,
        from_agent: &str,
        task: &str,
        thread_id: Option<uuid::Uuid>,
        constraints: HandoffConstraints,
        mode: HandoffMode,
    ) -> Result<Handoff> {
        let from_agent = from_agent.to_string();
        let task = task.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .create_handoff(wid, &from_agent, &task, thread_id, constraints, mode)
                .await
                .map_err(Into::into)
        })
    }

    pub fn get_guardrails(&self, thread_name: &str) -> Result<Option<ThreadGuardrails>> {
        self.run(self.store.get_guardrails(self.workspace_id, thread_name))
    }

    pub fn list_handoffs(
        &self,
        agent_name: Option<&str>,
        status: Option<HandoffStatus>,
    ) -> Result<Vec<Handoff>> {
        self.run(
            self.store
                .list_handoffs(self.workspace_id, agent_name, status),
        )
    }

    pub fn create_inbox_entry(
        &self,
        entry_type: InboxEntryType,
        title: &str,
        body: &str,
        severity: InboxSeverity,
        source_agent: Option<&str>,
        ref_type: Option<&str>,
        ref_id: Option<Uuid>,
    ) -> Result<InboxEntry> {
        let title = title.to_string();
        let body = body.to_string();
        let source_agent = source_agent.map(|s| s.to_string());
        let ref_type = ref_type.map(|s| s.to_string());
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .create_inbox_entry(
                    wid,
                    entry_type,
                    &title,
                    &body,
                    severity,
                    source_agent.as_deref(),
                    ref_type.as_deref(),
                    ref_id,
                )
                .await
                .map_err(Into::into)
        })
    }

    pub fn list_inbox_entries(&self, filters: InboxFilters) -> Result<Vec<InboxEntry>> {
        self.run(self.store.list_inbox_entries(self.workspace_id, filters))
    }

    pub fn dismiss_expired_entries(
        &self,
        entry_type: InboxEntryType,
        max_age_secs: u64,
    ) -> Result<u64> {
        self.run(
            self.store
                .dismiss_expired_entries(self.workspace_id, entry_type, max_age_secs),
        )
    }

    pub fn mark_inbox_read(&self, id: Uuid) -> Result<()> {
        self.run(self.store.mark_inbox_read(self.workspace_id, id))
    }

    pub fn dismiss_inbox_entry(&self, id: Uuid) -> Result<()> {
        self.run(self.store.dismiss_inbox_entry(self.workspace_id, id))
    }

    pub fn resolve_inbox_entry(&self, id: Uuid, resolution: &str) -> Result<()> {
        self.run(self.store.resolve_inbox_entry(self.workspace_id, id, resolution))
            .map(|_| ())
    }

    pub fn send_message(
        &self,
        from_agent: &str,
        to_agent: &str,
        content: &str,
    ) -> Result<prism_context::model::Message> {
        self.run(
            self.store
                .send_message(self.workspace_id, from_agent, to_agent, content, None),
        )
    }

    pub fn list_messages(
        &self,
        to_agent: &str,
        unread_only: bool,
    ) -> Result<Vec<prism_context::model::Message>> {
        self.run(
            self.store
                .list_messages(self.workspace_id, to_agent, unread_only),
        )
    }

    pub fn mark_messages_read(&self, to_agent: &str) -> Result<()> {
        self.run(self.store.mark_messages_read(self.workspace_id, to_agent))
    }

    pub fn count_unread_messages(&self, to_agent: &str) -> Result<i64> {
        self.run(
            self.store
                .count_unread_messages(self.workspace_id, to_agent),
        )
    }

    pub fn count_all_unread_messages(&self) -> Result<std::collections::HashMap<String, i64>> {
        self.run(self.store.count_all_unread_messages(self.workspace_id))
    }

    pub fn create_plan(&self, intent: &str) -> Result<Plan> {
        let intent = intent.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .create_plan(wid, &intent)
                .await
                .map_err(Into::into)
        })
    }

    pub fn get_plan(&self, plan_id: Uuid) -> Result<Plan> {
        self.run(self.store.get_plan(self.workspace_id, plan_id))
    }

    pub fn update_plan_status(&self, plan_id: Uuid, status: PlanStatus) -> Result<Plan> {
        self.run(
            self.store
                .update_plan_status(self.workspace_id, plan_id, status),
        )
    }

    pub fn list_plans(&self, status: Option<PlanStatus>) -> Result<Vec<Plan>> {
        self.run(self.store.list_plans(self.workspace_id, status))
    }

    pub fn get_active_plan(&self) -> Result<Option<Plan>> {
        self.run(self.store.get_active_plan(self.workspace_id))
    }

    pub fn list_change_sets(&self, plan_id: Option<Uuid>, wp_id: Option<Uuid>) -> Result<Vec<ChangeSet>> {
        self.run(self.store.list_change_sets(self.workspace_id, plan_id, wp_id))
    }

    pub fn update_plan_phase(&self, plan_id: Uuid, phase: MissionPhase) -> Result<Plan> {
        self.run(self.store.update_plan_phase(self.workspace_id, plan_id, phase))
    }

    pub fn update_plan_assumption_status(&self, plan_id: Uuid, index: usize, status: AssumptionStatus) -> Result<Plan> {
        self.run(self.store.update_plan_assumption(self.workspace_id, plan_id, index, status))
    }

    pub fn resolve_plan_blocker(&self, plan_id: Uuid, index: usize) -> Result<Plan> {
        self.run(self.store.resolve_plan_blocker(self.workspace_id, plan_id, index))
    }

    pub fn update_plan_metadata(&self, plan_id: Uuid, description: Option<&str>, constraints: Option<Vec<String>>, autonomy: Option<AutonomyLevel>) -> Result<Plan> {
        let description = description.map(|s| s.to_string());
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store.update_plan_metadata(wid, plan_id, description.as_deref(), constraints, autonomy)
                .await
                .map_err(Into::into)
        })
    }

    pub fn create_work_package(
        &self,
        plan_id: Option<Uuid>,
        intent: &str,
        acceptance_criteria: Vec<String>,
        ordinal: i32,
        depends_on: Vec<Uuid>,
        tags: Vec<String>,
    ) -> Result<WorkPackage> {
        let intent = intent.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .create_work_package(
                    wid,
                    plan_id,
                    &intent,
                    acceptance_criteria,
                    ordinal,
                    depends_on,
                    tags,
                )
                .await
                .map_err(Into::into)
        })
    }

    pub fn get_work_package(&self, wp_id: Uuid) -> Result<WorkPackage> {
        self.run(self.store.get_work_package(self.workspace_id, wp_id))
    }

    pub fn update_work_package_status(
        &self,
        wp_id: Uuid,
        status: WorkPackageStatus,
    ) -> Result<WorkPackage> {
        self.run(
            self.store
                .update_work_package_status(self.workspace_id, wp_id, status),
        )
    }

    pub fn assign_work_package(
        &self,
        wp_id: Uuid,
        agent_name: &str,
        thread_id: Uuid,
    ) -> Result<WorkPackage> {
        let agent_name = agent_name.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .assign_work_package(wid, wp_id, &agent_name, thread_id)
                .await
                .map_err(Into::into)
        })
    }

    pub fn list_work_packages(
        &self,
        plan_id: Option<Uuid>,
        status: Option<WorkPackageStatus>,
    ) -> Result<Vec<WorkPackage>> {
        self.run(
            self.store
                .list_work_packages(self.workspace_id, plan_id, status),
        )
    }

    pub fn refresh_work_package_readiness(&self, plan_id: Uuid) -> Result<Vec<WorkPackage>> {
        self.run(
            self.store
                .refresh_work_package_readiness(self.workspace_id, plan_id),
        )
    }

    pub fn list_risks(
        &self,
        status: Option<RiskStatus>,
        thread_id: Option<Uuid>,
    ) -> Result<Vec<Risk>> {
        self.run(self.store.list_risks(self.workspace_id, status, thread_id))
    }

    pub fn list_unverified_risks(&self, agent_name: Option<&str>) -> Result<Vec<Risk>> {
        let agent_name = agent_name.map(|s| s.to_string());
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .list_unverified_risks(wid, agent_name.as_deref())
                .await
                .map_err(Into::into)
        })
    }

    pub fn create_risk(
        &self,
        thread_id: Option<Uuid>,
        title: &str,
        description: &str,
        category: &str,
        severity: RiskSeverity,
        source_agent: Option<&str>,
        tags: Vec<String>,
    ) -> Result<Risk> {
        let title = title.to_string();
        let description = description.to_string();
        let category = category.to_string();
        let source_agent = source_agent.map(|s| s.to_string());
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .create_risk(
                    wid,
                    thread_id,
                    &title,
                    &description,
                    &category,
                    severity,
                    source_agent.as_deref(),
                    tags,
                )
                .await
                .map_err(Into::into)
        })
    }

    pub fn delete_memory(&self, key: &str) -> Result<()> {
        self.run(self.store.delete_memory(self.workspace_id, key))
    }

    pub fn load_memories(&self, filters: MemoryFilters) -> Result<Vec<Memory>> {
        self.run(self.store.load_memories(self.workspace_id, filters))
    }

    pub fn list_decisions(
        &self,
        thread_id: Option<Uuid>,
        tags: Option<Vec<String>>,
    ) -> Result<Vec<Decision>> {
        self.run(self.store.list_decisions(self.workspace_id, thread_id, tags))
    }

    pub fn supersede_decision(
        &self,
        old_id: Uuid,
        new_title: &str,
        new_content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
    ) -> Result<Decision> {
        let new_title = new_title.to_string();
        let new_content = new_content.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .supersede_decision(wid, old_id, &new_title, &new_content, thread_id, tags)
                .await
                .map_err(Into::into)
        })
    }

    pub fn revoke_decision(&self, id: Uuid) -> Result<Decision> {
        self.run(self.store.revoke_decision(self.workspace_id, id))
    }

    pub fn create_snapshot(&self, label: &str) -> Result<Snapshot> {
        self.run(self.store.create_snapshot(self.workspace_id, label))
    }

    pub fn list_snapshots(&self, limit: Option<i64>) -> Result<Vec<Snapshot>> {
        self.run(self.store.list_snapshots(self.workspace_id, limit))
    }

    pub fn recall_by_tags(
        &self,
        tags: Vec<String>,
        since: Option<DateTime<Utc>>,
    ) -> Result<RecallResult> {
        self.run(self.store.recall_by_tags(self.workspace_id, tags, since))
    }

    pub fn accept_handoff(&self, handoff_id: Uuid, agent_name: &str) -> Result<Handoff> {
        let agent_name = agent_name.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .accept_handoff(wid, handoff_id, &agent_name)
                .await
                .map_err(Into::into)
        })
    }

    pub fn complete_handoff(&self, handoff_id: Uuid, result: serde_json::Value) -> Result<Handoff> {
        self.run(self.store.complete_handoff(self.workspace_id, handoff_id, result))
    }

    pub fn start_handoff(&self, handoff_id: Uuid) -> Result<Handoff> {
        self.run(self.store.start_handoff(self.workspace_id, handoff_id))
    }

    pub fn fail_handoff(&self, handoff_id: Uuid, reason: &str) -> Result<Handoff> {
        let reason = reason.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .fail_handoff(wid, handoff_id, &reason)
                .await
                .map_err(Into::into)
        })
    }

    pub fn cancel_handoff(&self, handoff_id: Uuid) -> Result<Handoff> {
        self.run(self.store.cancel_handoff(self.workspace_id, handoff_id))
    }

    pub fn claim_file(&self, agent_name: &str, file_path: &str, ttl_secs: Option<i64>) -> Result<FileClaim> {
        let agent_name = agent_name.to_string();
        let file_path = file_path.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .claim_file(wid, &agent_name, &file_path, ttl_secs)
                .await
                .map_err(Into::into)
        })
    }

    pub fn check_file_claim(&self, file_path: &str) -> Result<Option<FileClaim>> {
        self.run(self.store.check_file_claim(self.workspace_id, file_path))
    }

    pub fn release_file(&self, file_path: &str, agent_name: &str) -> Result<()> {
        let file_path = file_path.to_string();
        let agent_name = agent_name.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .release_file(wid, &file_path, &agent_name)
                .await
                .map_err(Into::into)
        })
    }

    pub fn list_file_claims(&self, agent_name: Option<&str>) -> Result<Vec<FileClaim>> {
        let agent_name = agent_name.map(|s| s.to_string());
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .list_file_claims(wid, agent_name.as_deref())
                .await
                .map_err(Into::into)
        })
    }

    pub fn set_guardrails(
        &self,
        thread_name: &str,
        guardrails: ThreadGuardrails,
    ) -> Result<ThreadGuardrails> {
        let thread_name = thread_name.to_string();
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .set_guardrails(wid, &thread_name, guardrails)
                .await
                .map_err(Into::into)
        })
    }

    pub fn check_guardrail(
        &self,
        thread_name: &str,
        agent_name: &str,
        tool_name: &str,
        file_path: Option<&str>,
    ) -> Result<GuardrailCheck> {
        let thread_name = thread_name.to_string();
        let agent_name = agent_name.to_string();
        let tool_name = tool_name.to_string();
        let file_path = file_path.map(|s| s.to_string());
        let store = self.store.clone();
        let wid = self.workspace_id;
        self.handle.block_on(async move {
            store
                .check_guardrail(
                    wid,
                    &thread_name,
                    &agent_name,
                    &tool_name,
                    file_path.as_deref(),
                )
                .await
                .map_err(Into::into)
        })
    }
}

fn add_context_to_gitignore(workspace_root: &std::path::Path) {
    let gitignore_path = workspace_root.join(".gitignore");
    let entries = [".prism/context.json", ".prism/context.db", ".uglyhat.json", ".uglyhat.db"];
    let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    let existing: std::collections::HashSet<&str> = content.lines().map(|l| l.trim()).collect();
    let mut to_add = Vec::new();
    for entry in entries {
        if !existing.contains(entry) {
            to_add.push(entry);
        }
    }
    if !to_add.is_empty() {
        let suffix = to_add.join("\n");
        let updated = if content.is_empty() || content.ends_with('\n') {
            format!("{content}{suffix}\n")
        } else {
            format!("{content}\n{suffix}\n")
        };
        let _ = std::fs::write(&gitignore_path, updated);
    }
}

/// Helper to extract an `ContextHandle` from a `WeakEntity` context.
/// Use from within `cx.spawn(async move |this, cx| { ... })` closures.
pub fn get_context_handle<T: 'static>(
    this: &WeakEntity<T>,
    cx: &mut gpui::AsyncApp,
) -> Option<ContextHandle> {
    this.update(cx, |_, cx| {
        cx.try_global::<ContextService>()
            .and_then(|svc| svc.handle())
    })
    .ok()
    .flatten()
}
