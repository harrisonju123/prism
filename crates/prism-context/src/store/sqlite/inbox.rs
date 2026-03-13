use chrono::Utc;
use sqlx::Row as _;
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
                  source_agent, ref_type, ref_id, read, dismissed, resolved, created_at, updated_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,0,0,0,$10,$10)
             RETURNING id, workspace_id, entry_type, title, body, severity,
                       source_agent, ref_type, ref_id, read, dismissed, resolved, resolution,
                       created_at, updated_at",
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

    /// Find a recent entry matching entry_type + source_agent + title prefix within cooldown.
    pub(crate) async fn find_recent_similar_impl(
        &self,
        workspace_id: Uuid,
        entry_type: &InboxEntryType,
        source_agent: Option<&str>,
        title: &str,
        cooldown_secs: u64,
    ) -> Result<Option<String>> {
        let cutoff = Utc::now() - chrono::Duration::seconds(cooldown_secs as i64);
        let cutoff_str = cutoff.to_rfc3339();
        let title_prefix: String = title.chars().take(20).collect();

        let row = sqlx::query(
            "SELECT id FROM inbox_entries
             WHERE workspace_id = $1
               AND entry_type = $2
               AND (source_agent = $3 OR (source_agent IS NULL AND $3 IS NULL))
               AND dismissed = 0
               AND resolved = 0
               AND SUBSTR(title, 1, 20) = $4
               AND created_at > $5
             ORDER BY created_at DESC
             LIMIT 1",
        )
        .bind(workspace_id.to_string())
        .bind(entry_type.to_string())
        .bind(source_agent)
        .bind(&title_prefix)
        .bind(&cutoff_str)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| r.try_get::<String, _>("id").unwrap_or_default()))
    }

    /// Insert a new inbox entry or update the body/updated_at of a recent matching entry.
    pub(crate) async fn create_or_update_inbox_entry_impl(
        &self,
        workspace_id: Uuid,
        entry_type: InboxEntryType,
        title: &str,
        body: &str,
        severity: InboxSeverity,
        source_agent: Option<&str>,
        ref_type: Option<&str>,
        ref_id: Option<Uuid>,
        cooldown_secs: Option<u64>,
    ) -> Result<InboxEntry> {
        let cooldown = cooldown_secs.unwrap_or(300);

        if let Some(existing_id) =
            self.find_recent_similar_impl(workspace_id, &entry_type, source_agent, title, cooldown)
                .await?
        {
            let now = now_rfc3339();
            let updated_row = sqlx::query(
                "UPDATE inbox_entries SET body = $1, updated_at = $2
                 WHERE id = $3
                 RETURNING id, workspace_id, entry_type, title, body, severity,
                           source_agent, ref_type, ref_id, read, dismissed, resolved, resolution,
                           created_at, updated_at",
            )
            .bind(body)
            .bind(&now)
            .bind(&existing_id)
            .fetch_one(&self.pool)
            .await?;
            row_to_inbox_entry(&updated_row)
        } else {
            self.create_inbox_entry_impl(
                workspace_id,
                entry_type,
                title,
                body,
                severity,
                source_agent,
                ref_type,
                ref_id,
            )
            .await
        }
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
                    source_agent, ref_type, ref_id, read, dismissed, resolved, resolution,
                    created_at, updated_at
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

    pub(crate) async fn get_inbox_entry_impl(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<InboxEntry> {
        let row = sqlx::query(
            "SELECT id, workspace_id, entry_type, title, body, severity,
                    source_agent, ref_type, ref_id, read, dismissed, resolved, resolution,
                    created_at, updated_at
             FROM inbox_entries
             WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id.to_string())
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("inbox entry {id} not found")))?;

        row_to_inbox_entry(&row)
    }

    pub(crate) async fn resolve_inbox_entry_impl(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        resolution: &str,
    ) -> Result<InboxEntry> {
        let row = sqlx::query(
            "UPDATE inbox_entries SET resolved = 1, resolution = $1, read = 1
             WHERE workspace_id = $2 AND id = $3
             RETURNING id, workspace_id, entry_type, title, body, severity,
                       source_agent, ref_type, ref_id, read, dismissed, resolved, resolution,
                       created_at, updated_at",
        )
        .bind(resolution)
        .bind(workspace_id.to_string())
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("inbox entry {id} not found")))?;

        row_to_inbox_entry(&row)
    }

    /// Auto-dismiss unread, non-dismissed, non-resolved entries of `entry_type`
    /// that are older than `max_age_secs`. Returns the number of entries dismissed.
    pub(crate) async fn dismiss_expired_entries_impl(
        &self,
        workspace_id: Uuid,
        entry_type: InboxEntryType,
        max_age_secs: u64,
    ) -> Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::seconds(max_age_secs as i64);
        let cutoff_str = cutoff.to_rfc3339();

        let affected = sqlx::query(
            "UPDATE inbox_entries SET dismissed = 1, read = 1
             WHERE workspace_id = $1 AND entry_type = $2
               AND read = 0 AND dismissed = 0 AND resolved = 0
               AND created_at < $3",
        )
        .bind(workspace_id.to_string())
        .bind(entry_type.to_string())
        .bind(&cutoff_str)
        .execute(&self.pool)
        .await?
        .rows_affected();

        Ok(affected)
    }
}

/// Insert an inbox entry within an existing transaction.
pub(super) async fn insert_inbox_entry_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    workspace_id: Uuid,
    entry_type: InboxEntryType,
    title: &str,
    body: &str,
    severity: InboxSeverity,
    source_agent: Option<&str>,
    ref_type: Option<&str>,
    ref_id: Option<Uuid>,
) -> crate::error::Result<()> {
    let now = now_rfc3339();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO inbox_entries
             (id, workspace_id, entry_type, title, body, severity,
              source_agent, ref_type, ref_id, read, dismissed, resolved, created_at, updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,0,0,0,$10,$10)",
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
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Insert a new inbox entry within a transaction, or update body/updated_at of a recent match.
pub(super) async fn insert_or_update_inbox_entry_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    workspace_id: Uuid,
    entry_type: InboxEntryType,
    title: &str,
    body: &str,
    severity: InboxSeverity,
    source_agent: Option<&str>,
    ref_type: Option<&str>,
    ref_id: Option<Uuid>,
    cooldown_secs: Option<u64>,
) -> crate::error::Result<()> {
    let cooldown = cooldown_secs.unwrap_or(300);
    let cutoff = Utc::now() - chrono::Duration::seconds(cooldown as i64);
    let cutoff_str = cutoff.to_rfc3339();
    let title_prefix: String = title.chars().take(20).collect();

    let existing_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM inbox_entries
         WHERE workspace_id = $1
           AND entry_type = $2
           AND (source_agent = $3 OR (source_agent IS NULL AND $3 IS NULL))
           AND dismissed = 0
           AND resolved = 0
           AND SUBSTR(title, 1, 20) = $4
           AND created_at > $5
         ORDER BY created_at DESC
         LIMIT 1",
    )
    .bind(workspace_id.to_string())
    .bind(entry_type.to_string())
    .bind(source_agent)
    .bind(&title_prefix)
    .bind(&cutoff_str)
    .fetch_optional(&mut **tx)
    .await?;

    if let Some(id) = existing_id {
        let now = now_rfc3339();
        sqlx::query(
            "UPDATE inbox_entries SET body = $1, updated_at = $2 WHERE id = $3",
        )
        .bind(body)
        .bind(&now)
        .bind(&id)
        .execute(&mut **tx)
        .await?;
    } else {
        insert_inbox_entry_tx(
            tx,
            workspace_id,
            entry_type,
            title,
            body,
            severity,
            source_agent,
            ref_type,
            ref_id,
        )
        .await?;
    }

    Ok(())
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
        resolved: bool "resolved",
        resolution: custom "resolution" => {
            row.try_get::<Option<String>, _>("resolution")?
        },
        created_at: time "created_at",
        updated_at: custom "updated_at" => {
            let s: String = row.try_get::<String, _>("updated_at")?;
            if s.is_empty() {
                // Pre-migration rows: fall back to created_at
                let created: String = row.try_get::<String, _>("created_at")?;
                parse_time(&created)?
            } else {
                parse_time(&s)?
            }
        },
    }
}
