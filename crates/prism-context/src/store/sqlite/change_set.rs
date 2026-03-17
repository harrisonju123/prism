use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::Result;
use crate::model::*;

const CS_COLS: &str = "id, workspace_id, plan_id, wp_id, file_path, change_type, rationale, diff_excerpt, created_at, updated_at";

impl SqliteStore {
    pub(crate) async fn record_change_set_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Option<Uuid>,
        wp_id: Option<Uuid>,
        file_path: &str,
        change_type: ChangeType,
        rationale: &str,
        diff_excerpt: &str,
    ) -> Result<ChangeSet> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(&format!(
            "INSERT INTO change_sets (id, workspace_id, plan_id, wp_id, file_path, change_type, rationale, diff_excerpt, created_at, updated_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
             RETURNING {CS_COLS}",
        ))
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(plan_id.map(|u| u.to_string()))
        .bind(wp_id.map(|u| u.to_string()))
        .bind(file_path)
        .bind(change_type.to_string())
        .bind(rationale)
        .bind(diff_excerpt)
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        row_to_change_set(&row)
    }

    pub(crate) async fn list_change_sets_impl(
        &self,
        workspace_id: Uuid,
        plan_id: Option<Uuid>,
        wp_id: Option<Uuid>,
    ) -> Result<Vec<ChangeSet>> {
        let rows = match (plan_id, wp_id) {
            (Some(pid), Some(wid)) => {
                sqlx::query(&format!(
                    "SELECT {CS_COLS} FROM change_sets
                     WHERE workspace_id = $1 AND plan_id = $2 AND wp_id = $3
                     ORDER BY created_at DESC",
                ))
                .bind(workspace_id.to_string())
                .bind(pid.to_string())
                .bind(wid.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            (Some(pid), None) => {
                sqlx::query(&format!(
                    "SELECT {CS_COLS} FROM change_sets
                     WHERE workspace_id = $1 AND plan_id = $2
                     ORDER BY created_at DESC",
                ))
                .bind(workspace_id.to_string())
                .bind(pid.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            (None, Some(wid)) => {
                sqlx::query(&format!(
                    "SELECT {CS_COLS} FROM change_sets
                     WHERE workspace_id = $1 AND wp_id = $2
                     ORDER BY created_at DESC",
                ))
                .bind(workspace_id.to_string())
                .bind(wid.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            (None, None) => {
                sqlx::query(&format!(
                    "SELECT {CS_COLS} FROM change_sets
                     WHERE workspace_id = $1
                     ORDER BY created_at DESC",
                ))
                .bind(workspace_id.to_string())
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(row_to_change_set).collect()
    }
}

row_to_struct! {
    pub(super) fn row_to_change_set(row) -> ChangeSet {
        id: uuid "id",
        workspace_id: uuid "workspace_id",
        plan_id: opt_uuid "plan_id",
        wp_id: opt_uuid "wp_id",
        file_path: str "file_path",
        change_type: custom "change_type" => {
            let s: String = row.try_get::<String, _>("change_type")?;
            ChangeType::from_str(&s).unwrap_or_default()
        },
        rationale: custom "rationale" => {
            row.try_get::<String, _>("rationale").unwrap_or_default()
        },
        diff_excerpt: custom "diff_excerpt" => {
            row.try_get::<String, _>("diff_excerpt").unwrap_or_default()
        },
        created_at: time "created_at",
        updated_at: time "updated_at",
    }
}
