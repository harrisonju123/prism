use sqlx::Row;
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
             RETURNING id, workspace_id, thread_id, key, value, source, tags, created_at, updated_at",
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
            for tag in tags {
                args.push(tag.clone());
                clauses.push(format!("tags LIKE '%' || ${} || '%'", args.len()));
            }
        }

        let query = format!(
            "SELECT id, workspace_id, thread_id, key, value, source, tags, created_at, updated_at
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
        rows.iter().map(row_to_memory).collect()
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

pub(super) fn row_to_memory(row: &sqlx::sqlite::SqliteRow) -> Result<Memory> {
    let id_str: String = row.try_get("id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let thread_str: Option<String> = row.try_get("thread_id")?;
    let tags_str: String = row.try_get("tags")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Memory {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        thread_id: parse_opt_uuid(thread_str)?,
        key: row.try_get("key")?,
        value: row.try_get("value")?,
        source: row.try_get("source")?,
        tags: parse_json_array(&tags_str),
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}
