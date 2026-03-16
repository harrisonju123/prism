use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::Result;
use crate::model::*;
use crate::store::ActivityFilters;

impl SqliteStore {
    /// Log an activity entry. Errors are logged as warnings, never propagated.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn log_activity_fire_and_forget(
        &self,
        workspace_id: Uuid,
        actor: &str,
        action: &str,
        entity_type: &str,
        entity_id: Uuid,
        summary: &str,
        thread_id: Option<Uuid>,
    ) {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        if let Err(e) = sqlx::query(
            "INSERT INTO activity_log (id, workspace_id, actor, action, entity_type, entity_id, summary, thread_id, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(actor)
        .bind(action)
        .bind(entity_type)
        .bind(entity_id.to_string())
        .bind(summary)
        .bind(opt_uuid_to_str(thread_id))
        .bind(&now)
        .execute(&self.pool)
        .await
        {
            tracing::warn!(action, entity_type, %entity_id, "activity log insert failed: {e}");
        }
    }

    pub(crate) async fn list_activity_impl(
        &self,
        workspace_id: Uuid,
        filters: ActivityFilters,
    ) -> Result<Vec<ActivityEntry>> {
        let mut clauses = vec!["workspace_id = $1".to_string()];
        let mut args: Vec<String> = vec![workspace_id.to_string()];

        if let Some(ref since) = filters.since {
            args.push(since.to_rfc3339());
            clauses.push(format!("created_at >= ${}", args.len()));
        }
        if let Some(ref actor) = filters.actor {
            args.push(actor.clone());
            clauses.push(format!("actor = ${}", args.len()));
        }

        if let Some(tid) = filters.thread_id {
            args.push(tid.to_string());
            clauses.push(format!("thread_id = ${}", args.len()));
        }

        let limit = if filters.limit > 0 && filters.limit <= 200 {
            filters.limit
        } else {
            50
        };

        let query = format!(
            "SELECT id, workspace_id, actor, action, entity_type, entity_id, summary, detail, thread_id, created_at
             FROM activity_log
             WHERE {}
             ORDER BY created_at DESC
             LIMIT {}",
            clauses.join(" AND "),
            limit,
        );

        let mut q = sqlx::query(&query);
        for arg in &args {
            q = q.bind(arg);
        }

        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_activity_entry).collect()
    }
}

row_to_struct! {
    pub(super) fn row_to_activity_entry(row) -> ActivityEntry {
        id: uuid "id",
        workspace_id: uuid "workspace_id",
        actor: str "actor",
        action: str "action",
        entity_type: str "entity_type",
        entity_id: uuid "entity_id",
        summary: str "summary",
        detail: opt_json "detail",
        thread_id: opt_uuid "thread_id",
        created_at: time "created_at",
    }
}
