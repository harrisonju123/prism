use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui::{App, Global, WeakEntity};
use uglyhat::model::{
    ActivityEntry, AgentSession, AgentState, AgentStatus, CheckinContext, Decision, DecisionScope,
    Handoff, HandoffConstraints, HandoffMode, HandoffStatus, Memory, Thread, ThreadContext,
    ThreadGuardrails, ThreadStatus, WorkspaceOverview,
};
use uglyhat::store::sqlite::SqliteStore;
use uglyhat::store::{ActivityFilters, Store};
use uuid::Uuid;

/// GPUI Global that holds uglyhat state. Lives on the main thread.
/// Use `handle()` to get a cloneable, Send-able handle for background work.
pub struct UglyhatService {
    inner: Option<UglyhatHandle>,
}

impl Global for UglyhatService {}

/// Thread-safe, cloneable handle to uglyhat. Can be sent to background threads.
#[derive(Clone)]
pub struct UglyhatHandle {
    store: Arc<SqliteStore>,
    workspace_id: Uuid,
    handle: tokio::runtime::Handle,
}

impl UglyhatService {
    /// Initialize the service by discovering `.uglyhat.json` from `workspace_root`.
    /// Config discovery is synchronous; store opening is async (non-blocking).
    pub fn init(workspace_root: &std::path::Path, cx: &mut App) -> Result<()> {
        let tokio_handle = gpui_tokio::Tokio::handle(cx);

        let config_path = match uglyhat::config::find_config(workspace_root) {
            Some(path) => path,
            None => {
                // Only auto-init inside git repos to avoid false positives.
                if !workspace_root.join(".git").exists() {
                    anyhow::bail!("uglyhat not initialized in this workspace");
                }
                let workspace_name = workspace_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
                    .to_string();
                let dir = workspace_root.to_path_buf();
                let (path, _) = tokio_handle
                    .block_on(uglyhat::config::auto_init(&dir, &workspace_name))
                    .map_err(|e| anyhow::anyhow!("uglyhat auto-init: {e}"))?;
                add_uglyhat_to_gitignore(workspace_root);
                log::info!("uglyhat auto-initialized at {:?}", path);
                path
            }
        };

        let config =
            uglyhat::config::load_config(&config_path).map_err(|e| anyhow::anyhow!("{e}"))?;
        let workspace_id: Uuid = config
            .workspace_id
            .parse()
            .context("invalid workspace_id in .uglyhat.json")?;

        let db_path = uglyhat::config::resolve_db_path(&config_path, &config)
            .to_string_lossy()
            .to_string();

        // Set global immediately with no handle — callers already handle None gracefully
        cx.set_global(UglyhatService { inner: None });

        // Open the store on the tokio runtime, update global when ready
        let open_task = gpui_tokio::Tokio::spawn_result(cx, async move {
            use uglyhat::store::Store as _;
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
                    cx.global_mut::<UglyhatService>().inner = Some(UglyhatHandle {
                        store: Arc::new(store),
                        workspace_id,
                        handle: tokio_handle,
                    });
                });
            }
            Err(e) => {
                log::warn!("failed to open uglyhat store: {e}");
            }
        })
        .detach();

        Ok(())
    }

    /// Get a cloneable handle that can be sent to background threads.
    pub fn handle(&self) -> Option<UglyhatHandle> {
        self.inner.clone()
    }
}

impl UglyhatHandle {
    pub fn workspace_id(&self) -> Uuid {
        self.workspace_id
    }

    fn run<F, T>(&self, f: F) -> Result<T>
    where
        F: std::future::Future<Output = std::result::Result<T, uglyhat::error::Error>>,
    {
        self.handle.block_on(f).map_err(|e| anyhow::anyhow!("{e}"))
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
                .checkin(wid, &name, capabilities, thread_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
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
                .map_err(|e| anyhow::anyhow!("{e}"))
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
                .map_err(|e| anyhow::anyhow!("{e}"))
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
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
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
                .map_err(|e| anyhow::anyhow!("{e}"))
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
                .map_err(|e| anyhow::anyhow!("{e}"))
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
        self.run(self.store.list_handoffs(self.workspace_id, agent_name, status))
    }

    pub fn send_message(
        &self,
        from_agent: &str,
        to_agent: &str,
        content: &str,
    ) -> Result<uglyhat::model::Message> {
        self.run(self.store.send_message(self.workspace_id, from_agent, to_agent, content))
    }

    pub fn list_messages(
        &self,
        to_agent: &str,
        unread_only: bool,
    ) -> Result<Vec<uglyhat::model::Message>> {
        self.run(self.store.list_messages(self.workspace_id, to_agent, unread_only))
    }

    pub fn mark_messages_read(&self, to_agent: &str) -> Result<()> {
        self.run(self.store.mark_messages_read(self.workspace_id, to_agent))
    }

    pub fn count_unread_messages(&self, to_agent: &str) -> Result<i64> {
        self.run(self.store.count_unread_messages(self.workspace_id, to_agent))
    }

    pub fn count_all_unread_messages(&self) -> Result<std::collections::HashMap<String, i64>> {
        self.run(self.store.count_all_unread_messages(self.workspace_id))
    }
}

fn add_uglyhat_to_gitignore(workspace_root: &std::path::Path) {
    let gitignore_path = workspace_root.join(".gitignore");
    let entries = [".uglyhat.json", ".uglyhat.db"];
    let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    let mut to_add = Vec::new();
    for entry in entries {
        if !content.lines().any(|l| l.trim() == entry) {
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

/// Helper to extract an `UglyhatHandle` from a `WeakEntity` context.
/// Use from within `cx.spawn(async move |this, cx| { ... })` closures.
pub fn get_uglyhat_handle<T: 'static>(
    this: &WeakEntity<T>,
    cx: &mut gpui::AsyncApp,
) -> Option<UglyhatHandle> {
    this.update(cx, |_, cx| {
        cx.try_global::<UglyhatService>()
            .and_then(|svc| svc.handle())
    })
    .ok()
    .flatten()
}
