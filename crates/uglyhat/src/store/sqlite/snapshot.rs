use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::Result;
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn create_snapshot_impl(
        &self,
        workspace_id: Uuid,
        label: &str,
    ) -> Result<Snapshot> {
        let ws_str = workspace_id.to_string();

        // Build snapshot content from current state
        let threads = self
            .list_threads_impl(workspace_id, Some(ThreadStatus::Active))
            .await?;

        let memories = self
            .load_memories_impl(workspace_id, crate::store::MemoryFilters::default())
            .await?;

        let decisions = self.list_decisions_impl(workspace_id, None, None).await?;

        let agents = self.list_agents_impl(workspace_id).await?;

        let content = serde_json::json!({
            "threads": threads,
            "memories": memories,
            "decisions": decisions,
            "agents": agents,
        });

        let summary = format!(
            "{} threads, {} memories, {} decisions, {} agents",
            threads.len(),
            memories.len(),
            decisions.len(),
            agents.len(),
        );

        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO snapshots (id, workspace_id, label, summary, content, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, workspace_id, label, summary, content, created_at",
        )
        .bind(id.to_string())
        .bind(&ws_str)
        .bind(label)
        .bind(&summary)
        .bind(serde_json::to_string(&content).unwrap_or_else(|_| "{}".to_string()))
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        row_to_snapshot(&row)
    }
}

fn row_to_snapshot(row: &sqlx::sqlite::SqliteRow) -> Result<Snapshot> {
    let id_str: String = row.try_get("id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let content_str: String = row.try_get("content")?;
    let created_str: String = row.try_get("created_at")?;

    let content: serde_json::Value =
        serde_json::from_str(&content_str).unwrap_or(serde_json::json!({}));

    Ok(Snapshot {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        label: row.try_get("label")?,
        summary: row.try_get("summary")?,
        content,
        created_at: parse_time(&created_str)?,
    })
}
