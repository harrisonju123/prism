use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::Result;
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn set_guardrails_impl(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
        guardrails: ThreadGuardrails,
    ) -> Result<ThreadGuardrails> {
        let thread = self.get_thread_impl(workspace_id, thread_name).await?;
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let allowed_files = json_array_to_str(&guardrails.allowed_files);
        let allowed_tools = json_array_to_str(&guardrails.allowed_tools);

        let row = sqlx::query(
            "INSERT INTO thread_guardrails (id, thread_id, workspace_id, owner_agent_id, locked, allowed_files, allowed_tools, cost_budget_usd, cost_spent_usd, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (thread_id) DO UPDATE SET
                 owner_agent_id = excluded.owner_agent_id,
                 locked = excluded.locked,
                 allowed_files = excluded.allowed_files,
                 allowed_tools = excluded.allowed_tools,
                 cost_budget_usd = excluded.cost_budget_usd,
                 updated_at = excluded.updated_at
             RETURNING id, thread_id, workspace_id, owner_agent_id, locked, allowed_files, allowed_tools, cost_budget_usd, cost_spent_usd, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(thread.id.to_string())
        .bind(workspace_id.to_string())
        .bind(guardrails.owner_agent_id.map(|u| u.to_string()))
        .bind(guardrails.locked as i32)
        .bind(&allowed_files)
        .bind(&allowed_tools)
        .bind(guardrails.cost_budget_usd)
        .bind(guardrails.cost_spent_usd)
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        row_to_guardrails(&row)
    }

    pub(crate) async fn get_guardrails_impl(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
    ) -> Result<Option<ThreadGuardrails>> {
        let thread = self.get_thread_impl(workspace_id, thread_name).await?;

        let row = sqlx::query(
            "SELECT id, thread_id, workspace_id, owner_agent_id, locked, allowed_files, allowed_tools, cost_budget_usd, cost_spent_usd, created_at, updated_at
             FROM thread_guardrails
             WHERE thread_id = $1 AND workspace_id = $2",
        )
        .bind(thread.id.to_string())
        .bind(workspace_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(ref r) => Ok(Some(row_to_guardrails(r)?)),
            None => Ok(None),
        }
    }

    pub(crate) async fn check_guardrail_impl(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
        agent_name: &str,
        tool_name: &str,
        file_path: Option<&str>,
    ) -> Result<GuardrailCheck> {
        let thread = match self.get_thread_impl(workspace_id, thread_name).await {
            Ok(t) => t,
            Err(_) => {
                return Ok(GuardrailCheck {
                    allowed: true,
                    reason: None,
                });
            }
        };

        let row = sqlx::query(
            "SELECT id, thread_id, workspace_id, owner_agent_id, locked, allowed_files, allowed_tools, cost_budget_usd, cost_spent_usd, created_at, updated_at
             FROM thread_guardrails
             WHERE thread_id = $1 AND workspace_id = $2",
        )
        .bind(thread.id.to_string())
        .bind(workspace_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        let Some(ref r) = row else {
            return Ok(GuardrailCheck {
                allowed: true,
                reason: None,
            });
        };

        let guardrails = row_to_guardrails(r)?;

        // Check lock: only owner can operate on a locked thread
        if guardrails.locked
            && let Some(owner_id) = guardrails.owner_agent_id
        {
            let agent_row =
                sqlx::query("SELECT id FROM agents WHERE workspace_id = $1 AND name = $2")
                    .bind(workspace_id.to_string())
                    .bind(agent_name)
                    .fetch_optional(&self.pool)
                    .await?;

            let is_owner = agent_row
                .and_then(|r| r.try_get::<String, _>("id").ok())
                .and_then(|id| id.parse::<Uuid>().ok())
                .map(|id| id == owner_id)
                .unwrap_or(false);

            if !is_owner {
                return Ok(GuardrailCheck {
                    allowed: false,
                    reason: Some(format!(
                        "thread '{}' is locked by another agent",
                        thread_name
                    )),
                });
            }
        }

        // Check tool restrictions
        if !guardrails.allowed_tools.is_empty()
            && !guardrails.allowed_tools.iter().any(|t| t == tool_name)
        {
            return Ok(GuardrailCheck {
                allowed: false,
                reason: Some(format!(
                    "tool '{}' not in allowed tools for thread '{}'",
                    tool_name, thread_name
                )),
            });
        }

        // Check file restrictions
        if let Some(fp) = file_path
            && !guardrails.allowed_files.is_empty()
            && !guardrails
                .allowed_files
                .iter()
                .any(|pattern| path_matches_pattern(fp, pattern))
        {
            return Ok(GuardrailCheck {
                allowed: false,
                reason: Some(format!(
                    "file '{}' not in allowed files for thread '{}'",
                    fp, thread_name
                )),
            });
        }

        // Check cost budget
        if let Some(budget) = guardrails.cost_budget_usd
            && guardrails.cost_spent_usd >= budget
        {
            return Ok(GuardrailCheck {
                allowed: false,
                reason: Some(format!(
                    "cost budget exceeded for thread '{}': ${:.2} / ${:.2}",
                    thread_name, guardrails.cost_spent_usd, budget
                )),
            });
        }

        Ok(GuardrailCheck {
            allowed: true,
            reason: None,
        })
    }
}

/// Simple path-pattern matching: supports prefix matching and glob-style `*` suffix.
fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        path.starts_with(prefix)
    } else {
        path == pattern || path.starts_with(&format!("{pattern}/"))
    }
}

fn row_to_guardrails(row: &sqlx::sqlite::SqliteRow) -> Result<ThreadGuardrails> {
    use sqlx::Row as _;
    Ok(ThreadGuardrails {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        thread_id: parse_uuid(&row.try_get::<String, _>("thread_id")?)?,
        workspace_id: parse_uuid(&row.try_get::<String, _>("workspace_id")?)?,
        owner_agent_id: parse_opt_uuid(row.try_get("owner_agent_id")?)?,
        locked: row.try_get::<bool, _>("locked")?,
        allowed_files: parse_json_array(&row.try_get::<String, _>("allowed_files")?),
        allowed_tools: parse_json_array(&row.try_get::<String, _>("allowed_tools")?),
        cost_budget_usd: row.try_get("cost_budget_usd")?,
        cost_spent_usd: row.try_get::<f64, _>("cost_spent_usd")?,
        created_at: parse_time(&row.try_get::<String, _>("created_at")?)?,
        updated_at: parse_time(&row.try_get::<String, _>("updated_at")?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_matches_pattern() {
        assert!(path_matches_pattern("src/main.rs", "src/*"));
        assert!(path_matches_pattern("src/lib.rs", "src/*"));
        assert!(!path_matches_pattern("tests/main.rs", "src/*"));
        assert!(path_matches_pattern("src/main.rs", "src/main.rs"));
        assert!(!path_matches_pattern("src/lib.rs", "src/main.rs"));
        assert!(path_matches_pattern("src/foo/bar.rs", "src/foo"));
    }
}
