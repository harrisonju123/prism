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
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             RETURNING id, workspace_id, thread_id, title, content, status, tags, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(thread_id.map(|u| u.to_string()))
        .bind(title)
        .bind(content)
        .bind(DecisionStatus::Active.to_string())
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
            push_tag_clauses(tag_list, &mut clauses, &mut args);
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

row_to_struct! {
    pub(super) fn row_to_decision(row) -> Decision {
        id: uuid "id",
        workspace_id: uuid "workspace_id",
        thread_id: opt_uuid "thread_id",
        title: str "title",
        content: str "content",
        status: custom "status" => {
            let s: String = row.try_get::<String, _>("status")?;
            match s.as_str() {
                "active" => DecisionStatus::Active,
                other => return Err(crate::error::Error::Internal(format!("invalid decision status: {other}")))
            }
        },
        tags: json_array "tags",
        created_at: time "created_at",
        updated_at: time "updated_at",
    }
}
