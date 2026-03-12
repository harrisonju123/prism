use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;
use crate::store::MemoryFilters;

impl SqliteStore {
    pub(crate) async fn save_memory_impl(
        &self,
        workspace_id: Uuid,
        key: &str,
        value: &str,
        thread_id: Option<Uuid>,
        source: &str,
        tags: Vec<String>,
    ) -> Result<Memory> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();

        // Upsert on (workspace_id, key)
        let row = sqlx::query(
            "INSERT INTO memories (id, workspace_id, thread_id, key, value, source, tags, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (workspace_id, key) DO UPDATE
             SET value = excluded.value,
                 thread_id = excluded.thread_id,
                 source = excluded.source,
                 tags = excluded.tags,
                 updated_at = excluded.updated_at
             RETURNING id, workspace_id, thread_id, key, value, source, tags, access_count, last_accessed_at, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(thread_id.map(|u| u.to_string()))
        .bind(key)
        .bind(value)
        .bind(source)
        .bind(json_array_to_str(&tags))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        let memory = row_to_memory(&row)?;

        self.log_activity_fire_and_forget(
            workspace_id,
            source,
            "saved",
            "memory",
            memory.id,
            &format!("Saved memory: {key}"),
            None,
        )
        .await;

        Ok(memory)
    }

    pub(crate) async fn load_memories_impl(
        &self,
        workspace_id: Uuid,
        filters: MemoryFilters,
    ) -> Result<Vec<Memory>> {
        let mut clauses = vec!["workspace_id = $1".to_string()];
        let mut args: Vec<String> = vec![workspace_id.to_string()];

        if let Some(tid) = filters.thread_id {
            args.push(tid.to_string());
            clauses.push(format!("thread_id = ${}", args.len()));
        } else if let Some(ref tname) = filters.thread_name {
            args.push(tname.clone());
            clauses.push(format!(
                "thread_id = (SELECT id FROM threads WHERE workspace_id = $1 AND name = ${})",
                args.len()
            ));
        }

        if filters.global_only {
            clauses.push("thread_id IS NULL".to_string());
        }

        if let Some(ref tags) = filters.tags {
            push_tag_clauses(tags, &mut clauses, &mut args);
        }

        let query = format!(
            "SELECT id, workspace_id, thread_id, key, value, source, tags, access_count, last_accessed_at, created_at, updated_at
             FROM memories
             WHERE {}
             ORDER BY updated_at DESC
             LIMIT 500",
            clauses.join(" AND "),
        );

        let mut q = sqlx::query(&query);
        for arg in &args {
            q = q.bind(arg);
        }

        let rows = q.fetch_all(&self.pool).await?;
        let memories: Vec<Memory> = rows.iter().map(row_to_memory).collect::<Result<Vec<_>>>()?;

        // Track access for returned memories (fire-and-forget)
        if !memories.is_empty() {
            let ids: Vec<String> = memories.iter().map(|m| m.id.to_string()).collect();
            let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("${i}")).collect();
            let update_sql = format!(
                "UPDATE memories SET access_count = access_count + 1, last_accessed_at = ${} WHERE id IN ({})",
                ids.len() + 1,
                placeholders.join(", ")
            );
            let now = now_rfc3339();
            let mut update_q = sqlx::query(&update_sql);
            for id in &ids {
                update_q = update_q.bind(id);
            }
            update_q = update_q.bind(&now);
            let _ = update_q.execute(&self.pool).await;
        }

        Ok(memories)
    }

    pub(crate) async fn delete_memory_impl(&self, workspace_id: Uuid, key: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM memories WHERE workspace_id = $1 AND key = $2")
            .bind(workspace_id.to_string())
            .bind(key)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("memory {key:?} not found")));
        }
        Ok(())
    }
}

row_to_struct! {
    pub(super) fn row_to_memory(row) -> Memory {
        id: uuid "id",
        workspace_id: uuid "workspace_id",
        thread_id: opt_uuid "thread_id",
        key: str "key",
        value: str "value",
        source: str "source",
        tags: json_array "tags",
        access_count: custom "access_count" => row.try_get::<i32, _>("access_count").unwrap_or(0) as u32,
        last_accessed_at: opt_time "last_accessed_at",
        created_at: time "created_at",
        updated_at: time "updated_at",
    }
}
