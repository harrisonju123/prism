use chrono::{Datelike, Utc};
use dashmap::DashMap;

/// What to do when a budget is exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetAction {
    Reject,
    Warn,
}

impl BudgetAction {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "warn" => Self::Warn,
            _ => Self::Reject,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Warn => "warn",
        }
    }
}

/// Result of a budget check.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetCheckResult {
    Ok,
    Warning { message: String },
    Exceeded { message: String },
}

/// Per-key spend state.
#[derive(Debug, Clone)]
struct SpendState {
    daily_spend: f64,
    monthly_spend: f64,
    current_day: u32,   // day of year
    current_month: u32, // month number
    current_year: i32,
}

/// In-memory budget tracker.
pub struct BudgetTracker {
    state: DashMap<String, SpendState>,
}

impl BudgetTracker {
    pub fn new() -> Self {
        Self {
            state: DashMap::new(),
        }
    }

    /// Check whether the key is within its budget limits.
    pub fn check(
        &self,
        key_hash: &str,
        daily_limit: Option<f64>,
        monthly_limit: Option<f64>,
        action: BudgetAction,
    ) -> BudgetCheckResult {
        // No limits set → always OK
        if daily_limit.is_none() && monthly_limit.is_none() {
            return BudgetCheckResult::Ok;
        }

        let now = Utc::now();
        let today = now.ordinal();
        let month = now.month();
        let year = now.year();

        let mut entry = self
            .state
            .entry(key_hash.to_string())
            .or_insert_with(|| SpendState {
                daily_spend: 0.0,
                monthly_spend: 0.0,
                current_day: today,
                current_month: month,
                current_year: year,
            });

        // Auto-reset on day/month rollover
        if entry.current_year != year || entry.current_day != today {
            entry.daily_spend = 0.0;
            entry.current_day = today;
        }
        if entry.current_year != year || entry.current_month != month {
            entry.monthly_spend = 0.0;
            entry.current_month = month;
            entry.current_year = year;
        }

        // Check daily
        if let Some(limit) = daily_limit
            && entry.daily_spend >= limit
        {
            return match action {
                BudgetAction::Reject => BudgetCheckResult::Exceeded {
                    message: format!(
                        "daily budget exceeded: ${:.4} / ${:.2}",
                        entry.daily_spend, limit
                    ),
                },
                BudgetAction::Warn => BudgetCheckResult::Warning {
                    message: format!(
                        "daily budget exceeded: ${:.4} / ${:.2}",
                        entry.daily_spend, limit
                    ),
                },
            };
        }

        // Check monthly
        if let Some(limit) = monthly_limit
            && entry.monthly_spend >= limit
        {
            return match action {
                BudgetAction::Reject => BudgetCheckResult::Exceeded {
                    message: format!(
                        "monthly budget exceeded: ${:.4} / ${:.2}",
                        entry.monthly_spend, limit
                    ),
                },
                BudgetAction::Warn => BudgetCheckResult::Warning {
                    message: format!(
                        "monthly budget exceeded: ${:.4} / ${:.2}",
                        entry.monthly_spend, limit
                    ),
                },
            };
        }

        BudgetCheckResult::Ok
    }

    /// Record spend for a completed request.
    pub fn record_spend(&self, key_hash: &str, cost_usd: f64) {
        if cost_usd <= 0.0 {
            return;
        }
        let now = Utc::now();
        let today = now.ordinal();
        let month = now.month();
        let year = now.year();

        let mut entry = self
            .state
            .entry(key_hash.to_string())
            .or_insert_with(|| SpendState {
                daily_spend: 0.0,
                monthly_spend: 0.0,
                current_day: today,
                current_month: month,
                current_year: year,
            });

        // Reset on rollover
        if entry.current_year != year || entry.current_day != today {
            entry.daily_spend = 0.0;
            entry.current_day = today;
        }
        if entry.current_year != year || entry.current_month != month {
            entry.monthly_spend = 0.0;
            entry.current_month = month;
            entry.current_year = year;
        }

        entry.daily_spend += cost_usd;
        entry.monthly_spend += cost_usd;
    }

    /// Reconcile in-memory state with an external source (e.g., ClickHouse).
    /// Takes the max of in-memory vs external to prevent under-counting.
    pub fn reconcile(&self, key_hash: &str, daily_total: f64, monthly_total: f64) {
        let now = Utc::now();
        let today = now.ordinal();
        let month = now.month();
        let year = now.year();

        let mut entry = self
            .state
            .entry(key_hash.to_string())
            .or_insert_with(|| SpendState {
                daily_spend: 0.0,
                monthly_spend: 0.0,
                current_day: today,
                current_month: month,
                current_year: year,
            });

        if entry.current_year != year || entry.current_day != today {
            entry.daily_spend = 0.0;
            entry.current_day = today;
        }
        if entry.current_year != year || entry.current_month != month {
            entry.monthly_spend = 0.0;
            entry.current_month = month;
            entry.current_year = year;
        }

        entry.daily_spend = entry.daily_spend.max(daily_total);
        entry.monthly_spend = entry.monthly_spend.max(monthly_total);
    }

    /// Check budget with hierarchy support. Walks up: key → user → team → org.
    pub fn check_hierarchy(
        &self,
        key_hash: &str,
        daily_limit: Option<f64>,
        monthly_limit: Option<f64>,
        action: BudgetAction,
        hierarchy: Option<&BudgetHierarchy>,
    ) -> BudgetCheckResult {
        // First check the key's own budget
        let result = self.check(key_hash, daily_limit, monthly_limit, action);
        if !matches!(result, BudgetCheckResult::Ok) {
            return result;
        }

        // Then check parent budgets
        if let Some(hierarchy) = hierarchy {
            for node in &hierarchy.ancestors {
                let node_hash = format!("{}:{}", node.node_type, node.node_id);
                let node_result = self.check(
                    &node_hash,
                    node.daily_budget_usd,
                    node.monthly_budget_usd,
                    BudgetAction::from_str_lossy(&node.budget_action),
                );
                if !matches!(node_result, BudgetCheckResult::Ok) {
                    return node_result;
                }
            }
        }

        BudgetCheckResult::Ok
    }

    /// Record spend across the budget hierarchy.
    pub fn record_spend_hierarchy(
        &self,
        key_hash: &str,
        cost_usd: f64,
        hierarchy: Option<&BudgetHierarchy>,
    ) {
        self.record_spend(key_hash, cost_usd);

        if let Some(hierarchy) = hierarchy {
            for node in &hierarchy.ancestors {
                let node_hash = format!("{}:{}", node.node_type, node.node_id);
                self.record_spend(&node_hash, cost_usd);
            }
        }
    }

    /// Get current spend for a key (for usage reporting).
    pub fn get_spend(&self, key_hash: &str) -> (f64, f64) {
        let now = Utc::now();
        let today = now.ordinal();
        let month = now.month();
        let year = now.year();

        self.state
            .get(key_hash)
            .map(|s| {
                let daily = if s.current_year == year && s.current_day == today {
                    s.daily_spend
                } else {
                    0.0
                };
                let monthly = if s.current_year == year && s.current_month == month {
                    s.monthly_spend
                } else {
                    0.0
                };
                (daily, monthly)
            })
            .unwrap_or((0.0, 0.0))
    }
}

// ---------------------------------------------------------------------------
// Budget Hierarchy types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BudgetNode {
    pub id: uuid::Uuid,
    pub parent_id: Option<uuid::Uuid>,
    pub node_type: String,
    pub node_id: String,
    pub daily_budget_usd: Option<f64>,
    pub monthly_budget_usd: Option<f64>,
    pub budget_action: String,
}

#[derive(Debug, Clone, Default)]
pub struct BudgetHierarchy {
    pub ancestors: Vec<BudgetNode>,
}

#[cfg(feature = "postgres")]
impl BudgetHierarchy {
    pub async fn load_for_key(
        pool: &sqlx::PgPool,
        key_team_id: Option<&str>,
    ) -> Result<Self, sqlx::Error> {
        let mut ancestors = Vec::new();

        // If key has a team_id, load the team node and walk up
        if let Some(team_id) = key_team_id {
            let nodes: Vec<BudgetNodeRow> = sqlx::query_as(
                r#"WITH RECURSIVE hierarchy AS (
                    SELECT id, parent_id, node_type, node_id, daily_budget_usd, monthly_budget_usd, budget_action
                    FROM budget_nodes WHERE node_type = 'team' AND node_id = $1
                    UNION ALL
                    SELECT bn.id, bn.parent_id, bn.node_type, bn.node_id, bn.daily_budget_usd, bn.monthly_budget_usd, bn.budget_action
                    FROM budget_nodes bn
                    JOIN hierarchy h ON bn.id = h.parent_id
                )
                SELECT id, parent_id, node_type, node_id, daily_budget_usd, monthly_budget_usd, budget_action
                FROM hierarchy"#,
            )
            .bind(team_id)
            .fetch_all(pool)
            .await?;

            ancestors = nodes.into_iter().map(Into::into).collect();
        }

        Ok(Self { ancestors })
    }
}

#[cfg(feature = "postgres")]
#[derive(Debug, sqlx::FromRow)]
struct BudgetNodeRow {
    id: uuid::Uuid,
    parent_id: Option<uuid::Uuid>,
    node_type: String,
    node_id: String,
    daily_budget_usd: Option<f64>,
    monthly_budget_usd: Option<f64>,
    budget_action: String,
}

#[cfg(feature = "postgres")]
impl From<BudgetNodeRow> for BudgetNode {
    fn from(row: BudgetNodeRow) -> Self {
        Self {
            id: row.id,
            parent_id: row.parent_id,
            node_type: row.node_type,
            node_id: row.node_id,
            daily_budget_usd: row.daily_budget_usd,
            monthly_budget_usd: row.monthly_budget_usd,
            budget_action: row.budget_action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_under_daily_limit() {
        let bt = BudgetTracker::new();
        bt.record_spend("k1", 0.50);
        let result = bt.check("k1", Some(1.00), None, BudgetAction::Reject);
        assert_eq!(result, BudgetCheckResult::Ok);
    }

    #[test]
    fn rejects_over_daily_limit() {
        let bt = BudgetTracker::new();
        bt.record_spend("k1", 1.50);
        let result = bt.check("k1", Some(1.00), None, BudgetAction::Reject);
        assert!(matches!(result, BudgetCheckResult::Exceeded { .. }));
    }

    #[test]
    fn warns_over_daily_limit() {
        let bt = BudgetTracker::new();
        bt.record_spend("k1", 1.50);
        let result = bt.check("k1", Some(1.00), None, BudgetAction::Warn);
        assert!(matches!(result, BudgetCheckResult::Warning { .. }));
    }

    #[test]
    fn allows_under_monthly_limit() {
        let bt = BudgetTracker::new();
        bt.record_spend("k1", 5.00);
        let result = bt.check("k1", None, Some(10.00), BudgetAction::Reject);
        assert_eq!(result, BudgetCheckResult::Ok);
    }

    #[test]
    fn rejects_over_monthly_limit() {
        let bt = BudgetTracker::new();
        bt.record_spend("k1", 15.00);
        let result = bt.check("k1", None, Some(10.00), BudgetAction::Reject);
        assert!(matches!(result, BudgetCheckResult::Exceeded { .. }));
    }

    #[test]
    fn no_limits_always_ok() {
        let bt = BudgetTracker::new();
        bt.record_spend("k1", 999.0);
        let result = bt.check("k1", None, None, BudgetAction::Reject);
        assert_eq!(result, BudgetCheckResult::Ok);
    }

    #[test]
    fn reconcile_takes_max() {
        let bt = BudgetTracker::new();
        bt.record_spend("k1", 5.00);
        bt.reconcile("k1", 8.00, 20.00);
        let (daily, monthly) = bt.get_spend("k1");
        assert!((daily - 8.00).abs() < f64::EPSILON);
        assert!((monthly - 20.00).abs() < f64::EPSILON);
    }

    #[test]
    fn reconcile_keeps_higher_in_memory() {
        let bt = BudgetTracker::new();
        bt.record_spend("k1", 10.00);
        bt.reconcile("k1", 5.00, 5.00);
        let (daily, monthly) = bt.get_spend("k1");
        assert!((daily - 10.00).abs() < f64::EPSILON);
        assert!((monthly - 10.00).abs() < f64::EPSILON);
    }

    #[test]
    fn budget_action_parsing() {
        assert_eq!(BudgetAction::from_str_lossy("warn"), BudgetAction::Warn);
        assert_eq!(BudgetAction::from_str_lossy("reject"), BudgetAction::Reject);
        assert_eq!(
            BudgetAction::from_str_lossy("unknown"),
            BudgetAction::Reject
        );
    }

    #[test]
    fn hierarchy_check_passes_when_all_ok() {
        let bt = BudgetTracker::new();
        bt.record_spend("k1", 1.0);
        let hierarchy = BudgetHierarchy {
            ancestors: vec![BudgetNode {
                id: uuid::Uuid::new_v4(),
                parent_id: None,
                node_type: "team".into(),
                node_id: "team1".into(),
                daily_budget_usd: Some(100.0),
                monthly_budget_usd: Some(1000.0),
                budget_action: "reject".into(),
            }],
        };
        let result = bt.check_hierarchy(
            "k1",
            Some(10.0),
            None,
            BudgetAction::Reject,
            Some(&hierarchy),
        );
        assert_eq!(result, BudgetCheckResult::Ok);
    }

    #[test]
    fn hierarchy_check_fails_on_parent_exceeded() {
        let bt = BudgetTracker::new();
        bt.record_spend("k1", 1.0);
        bt.record_spend("team:team1", 200.0); // parent over budget
        let hierarchy = BudgetHierarchy {
            ancestors: vec![BudgetNode {
                id: uuid::Uuid::new_v4(),
                parent_id: None,
                node_type: "team".into(),
                node_id: "team1".into(),
                daily_budget_usd: Some(100.0),
                monthly_budget_usd: None,
                budget_action: "reject".into(),
            }],
        };
        let result = bt.check_hierarchy(
            "k1",
            Some(10.0),
            None,
            BudgetAction::Reject,
            Some(&hierarchy),
        );
        assert!(matches!(result, BudgetCheckResult::Exceeded { .. }));
    }

    #[test]
    fn record_spend_hierarchy() {
        let bt = BudgetTracker::new();
        let hierarchy = BudgetHierarchy {
            ancestors: vec![BudgetNode {
                id: uuid::Uuid::new_v4(),
                parent_id: None,
                node_type: "org".into(),
                node_id: "org1".into(),
                daily_budget_usd: Some(1000.0),
                monthly_budget_usd: None,
                budget_action: "reject".into(),
            }],
        };
        bt.record_spend_hierarchy("k1", 5.0, Some(&hierarchy));
        let (daily, _) = bt.get_spend("k1");
        assert!((daily - 5.0).abs() < f64::EPSILON);
        let (org_daily, _) = bt.get_spend("org:org1");
        assert!((org_daily - 5.0).abs() < f64::EPSILON);
    }
}
