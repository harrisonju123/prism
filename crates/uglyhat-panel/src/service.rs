use std::sync::Arc;

use anyhow::{Context as _, Result};
use gpui::{App, Global, WeakEntity};
use uglyhat::model::{
    ActivityEntry, AgentSession, AgentStatus, CheckinContext, Memory, Thread, WorkspaceOverview,
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
        let config_path = uglyhat::config::find_config(workspace_root)
            .context("uglyhat not initialized in this workspace")?;
        let config =
            uglyhat::config::load_config(&config_path).map_err(|e| anyhow::anyhow!("{e}"))?;
        let workspace_id: Uuid = config
            .workspace_id
            .parse()
            .context("invalid workspace_id in .uglyhat.json")?;

        let db_path = uglyhat::config::resolve_db_path(&config_path, &config)
            .to_string_lossy()
            .to_string();

        let tokio_handle = gpui_tokio::Tokio::handle(cx);

        // Set global immediately with no handle — callers already handle None gracefully
        cx.set_global(UglyhatService { inner: None });

        // Open the store on the tokio runtime, update global when ready
        let open_task = gpui_tokio::Tokio::spawn_result(cx, async move {
            SqliteStore::open(&db_path)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
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
