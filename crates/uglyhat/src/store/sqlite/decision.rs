use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::Result;
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn save_decision_impl(
        &self,
        workspace_id: Uuid,
        title: &str,
        content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
    ) -> Result<Decision> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO decisions (id, workspace_id, thread_id, title, content, status, tags, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, 'active', $6, $7, $8)
             RETURNING id, workspace_id, thread_id, title, content, status, tags, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(thread_id.map(|u| u.to_string()))
        .bind(title)
        .bind(content)
        .bind(json_array_to_str(&tags))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        let decision = row_to_decision(&row)?;

        self.log_activity_fire_and_forget(
                workspace_id,
                "",
                "decided",
                "decision",
                decision.id,
                &format!("Decision: {title}"),
                None,
            )
            .await;

        Ok(decision)
    }

    pub(crate) async fn list_decisions_impl(
        &self,
        workspace_id: Uuid,
        thread_id: Option<Uuid>,
        tags: Option<Vec<String>>,
    ) -> Result<Vec<Decision>> {
        let mut clauses = vec!["workspace_id = $1".to_string()];
        let mut args: Vec<String> = vec![workspace_id.to_string()];

        if let Some(tid) = thread_id {
            args.push(tid.to_string());
            clauses.push(format!("thread_id = ${}", args.len()));
        }

        if let Some(ref tag_list) = tags {
            for tag in tag_list {
                args.push(tag.clone());
                clauses.push(format!("tags LIKE '%' || ${} || '%'", args.len()));
            }
        }

        let query = format!(
            "SELECT id, workspace_id, thread_id, title, content, status, tags, created_at, updated_at
             FROM decisions
             WHERE {}
             ORDER BY created_at DESC
             LIMIT 200",
            clauses.join(" AND "),
        );

        let mut q = sqlx::query(&query);
        for arg in &args {
            q = q.bind(arg);
        }

        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_decision).collect()
    }
}

pub(super) fn row_to_decision(row: &sqlx::sqlite::SqliteRow) -> Result<Decision> {
    let id_str: String = row.try_get("id")?;
    let ws_str: String = row.try_get("workspace_id")?;
    let thread_str: Option<String> = row.try_get("thread_id")?;
    let tags_str: String = row.try_get("tags")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Decision {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_uuid(&ws_str)?,
        thread_id: parse_opt_uuid(thread_str)?,
        title: row.try_get("title")?,
        content: row.try_get("content")?,
        status: row.try_get("status")?,
        tags: parse_json_array(&tags_str),
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}
