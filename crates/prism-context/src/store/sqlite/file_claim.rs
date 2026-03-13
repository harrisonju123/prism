use sqlx::Row as _;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::FileClaim;

impl SqliteStore {
    /// Prune expired claims for a workspace — fire-and-forget, runs before reads/writes.
    async fn prune_expired_claims(&self, workspace_id: Uuid) {
        let now = now_rfc3339();
        if let Err(e) = sqlx::query(
            "DELETE FROM file_claims WHERE workspace_id = $1 AND expires_at IS NOT NULL AND expires_at < $2",
        )
        .bind(workspace_id.to_string())
        .bind(&now)
        .execute(&self.pool)
        .await
        {
            tracing::warn!(%workspace_id, "prune expired file_claims failed: {e}");
        }
    }

    pub(crate) async fn claim_file_impl(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        file_path: &str,
        ttl_secs: Option<i64>,
    ) -> Result<FileClaim> {
        self.prune_expired_claims(workspace_id).await;

        let id = Uuid::new_v4();
        let now = now_rfc3339();
        let expires_at =
            ttl_secs.map(|s| (chrono::Utc::now() + chrono::Duration::seconds(s)).to_rfc3339());

        // Wrap in a transaction so the SELECT-then-INSERT is atomic under SQLite WAL.
        let mut tx = self.pool.begin().await?;

        // Check for an existing live claim before attempting insert.
        let existing = sqlx::query(
            "SELECT id, workspace_id, file_path, agent_name, claimed_at, expires_at
             FROM file_claims WHERE workspace_id = $1 AND file_path = $2",
        )
        .bind(workspace_id.to_string())
        .bind(file_path)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(row) = existing {
            tx.rollback().await?;
            let claim = row_to_file_claim(&row)?;
            if claim.agent_name != agent_name {
                return Err(Error::Conflict(
                    serde_json::to_string(&claim).unwrap_or_else(|_| claim.agent_name.clone()),
                ));
            }
            // Same agent — re-claim is a no-op, return existing.
            return Ok(claim);
        }

        let insert_result = sqlx::query(
            "INSERT INTO file_claims (id, workspace_id, file_path, agent_name, claimed_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(file_path)
        .bind(agent_name)
        .bind(&now)
        .bind(&expires_at)
        .execute(&mut *tx)
        .await;

        match insert_result {
            Ok(_) => {}
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                tx.rollback().await?;
                return Err(Error::Conflict(format!(
                    "file '{file_path}' already claimed by another agent"
                )));
            }
            Err(e) => return Err(e.into()),
        }

        tx.commit().await?;

        self.log_activity_fire_and_forget(
            workspace_id,
            agent_name,
            "claim",
            "file_claim",
            id,
            &format!("claimed {file_path}"),
            None,
        )
        .await;

        Ok(FileClaim {
            id,
            workspace_id,
            file_path: file_path.to_string(),
            agent_name: agent_name.to_string(),
            claimed_at: parse_time(&now)?,
            expires_at: expires_at.as_deref().and_then(|s| parse_time(s).ok()),
        })
    }

    pub(crate) async fn release_file_impl(
        &self,
        workspace_id: Uuid,
        file_path: &str,
        agent_name: &str,
    ) -> Result<()> {
        let deleted_id: Option<String> = sqlx::query_scalar(
            "DELETE FROM file_claims WHERE workspace_id = $1 AND file_path = $2 AND agent_name = $3 RETURNING id",
        )
        .bind(workspace_id.to_string())
        .bind(file_path)
        .bind(agent_name)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(id_str) = deleted_id {
            if let Ok(id) = parse_uuid(&id_str) {
                self.log_activity_fire_and_forget(
                    workspace_id,
                    agent_name,
                    "release",
                    "file_claim",
                    id,
                    &format!("released {file_path}"),
                    None,
                )
                .await;
            }
        }

        Ok(())
    }

    pub(crate) async fn check_file_claim_impl(
        &self,
        workspace_id: Uuid,
        file_path: &str,
    ) -> Result<Option<FileClaim>> {
        self.prune_expired_claims(workspace_id).await;

        let row = sqlx::query(
            "SELECT id, workspace_id, file_path, agent_name, claimed_at, expires_at
             FROM file_claims WHERE workspace_id = $1 AND file_path = $2",
        )
        .bind(workspace_id.to_string())
        .bind(file_path)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| row_to_file_claim(&r)).transpose()
    }

    pub(crate) async fn list_file_claims_impl(
        &self,
        workspace_id: Uuid,
        agent_name: Option<&str>,
    ) -> Result<Vec<FileClaim>> {
        self.prune_expired_claims(workspace_id).await;

        let rows = if let Some(name) = agent_name {
            sqlx::query(
                "SELECT id, workspace_id, file_path, agent_name, claimed_at, expires_at
                 FROM file_claims WHERE workspace_id = $1 AND agent_name = $2
                 ORDER BY claimed_at ASC",
            )
            .bind(workspace_id.to_string())
            .bind(name)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, workspace_id, file_path, agent_name, claimed_at, expires_at
                 FROM file_claims WHERE workspace_id = $1
                 ORDER BY claimed_at ASC",
            )
            .bind(workspace_id.to_string())
            .fetch_all(&self.pool)
            .await?
        };

        rows.iter().map(row_to_file_claim).collect()
    }
}

fn row_to_file_claim(row: &sqlx::sqlite::SqliteRow) -> Result<FileClaim> {
    Ok(FileClaim {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        workspace_id: parse_uuid(&row.try_get::<String, _>("workspace_id")?)?,
        file_path: row.try_get("file_path")?,
        agent_name: row.try_get("agent_name")?,
        claimed_at: parse_time(&row.try_get::<String, _>("claimed_at")?)?,
        expires_at: parse_opt_time(row.try_get::<Option<String>, _>("expires_at")?)?,
    })
}
