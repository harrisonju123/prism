use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;
use crate::store::InboxFilters;

impl SqliteStore {
    pub(crate) async fn create_inbox_entry_impl(
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
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO inbox_entries
                 (id, workspace_id, entry_type, title, body, severity,
                  source_agent, ref_type, ref_id, read, dismissed, created_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,0,0,$10)
             RETURNING id, workspace_id, entry_type, title, body, severity,
                       source_agent, ref_type, ref_id, read, dismissed, created_at",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(entry_type.to_string())
        .bind(title)
        .bind(body)
        .bind(severity.to_string())
        .bind(source_agent)
        .bind(ref_type)
        .bind(ref_id.map(|u| u.to_string()))
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        row_to_inbox_entry(&row)
    }

    pub(crate) async fn list_inbox_entries_impl(
        &self,
        workspace_id: Uuid,
        filters: InboxFilters,
    ) -> Result<Vec<InboxEntry>> {
        let limit = filters.limit.clamp(1, 500);
        let mut clauses: Vec<String> = vec!["workspace_id = $1".to_string()];
        let mut args: Vec<String> = vec![workspace_id.to_string()];

        if !filters.include_dismissed {
            clauses.push("dismissed = 0".to_string());
        }
        if filters.unread_only {
            clauses.push("read = 0".to_string());
        }
        if let Some(ref et) = filters.entry_type {
            args.push(et.to_string());
            clauses.push(format!("entry_type = ${}", args.len()));
        }

        let where_clause = clauses.join(" AND ");
        let sql = format!(
            "SELECT id, workspace_id, entry_type, title, body, severity,
                    source_agent, ref_type, ref_id, read, dismissed, created_at
             FROM inbox_entries
             WHERE {where_clause}
             ORDER BY created_at DESC
             LIMIT {limit}"
        );

        let mut q = sqlx::query(&sql);
        for arg in &args {
            q = q.bind(arg.as_str());
        }

        let rows = q.fetch_all(&self.pool).await?;
        rows.iter().map(row_to_inbox_entry).collect()
    }

    pub(crate) async fn mark_inbox_read_impl(&self, workspace_id: Uuid, id: Uuid) -> Result<()> {
        let affected = sqlx::query(
            "UPDATE inbox_entries SET read = 1
             WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id.to_string())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?
        .rows_affected();

        if affected == 0 {
            return Err(Error::NotFound(format!("inbox entry {id} not found")));
        }
        Ok(())
    }

    pub(crate) async fn dismiss_inbox_entry_impl(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<()> {
        let affected = sqlx::query(
            "UPDATE inbox_entries SET dismissed = 1, read = 1
             WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id.to_string())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?
        .rows_affected();

        if affected == 0 {
            return Err(Error::NotFound(format!("inbox entry {id} not found")));
        }
        Ok(())
    }
}

row_to_struct! {
    pub(super) fn row_to_inbox_entry(row) -> InboxEntry {
        id: uuid "id",
        workspace_id: uuid "workspace_id",
        entry_type: custom "entry_type" => {
            let s: String = row.try_get::<String, _>("entry_type")?;
            InboxEntryType::from_str(&s)
                .ok_or_else(|| Error::Internal(format!("unknown inbox entry_type: {s:?}")))?
        },
        title: str "title",
        body: str "body",
        severity: custom "severity" => {
            let s: String = row.try_get::<String, _>("severity")?;
            InboxSeverity::from_str(&s)
                .ok_or_else(|| Error::Internal(format!("unknown inbox severity: {s:?}")))?
        },
        source_agent: custom "source_agent" => {
            row.try_get::<Option<String>, _>("source_agent")?
        },
        ref_type: custom "ref_type" => {
            row.try_get::<Option<String>, _>("ref_type")?
        },
        ref_id: opt_uuid "ref_id",
        read: bool "read",
        dismissed: bool "dismissed",
        created_at: time "created_at",
    }
}
