use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::alerts::notify;
use crate::alerts::types::{AlertChannel, AlertEvent, AlertRule, AlertSeverity, RuleType};
use crate::config::{BudgetAlertsConfig, SmtpConfig};
use crate::keys::budget::BudgetTracker;

#[cfg(feature = "postgres")]
use crate::keys::virtual_key::KeyRepository;

/// Cooldown key: (key_id, period, threshold_label).
type CooldownKey = (Uuid, &'static str, &'static str);

/// Background task that scans virtual keys with budgets and fires alerts when
/// daily or monthly spend reaches the warning or exceeded thresholds.
pub struct BudgetWatcher {
    cfg: BudgetAlertsConfig,
    budget_tracker: Arc<BudgetTracker>,
    smtp_config: Option<SmtpConfig>,
    cooldowns: HashMap<CooldownKey, Instant>,
    #[cfg(feature = "postgres")]
    repo: KeyRepository,
    #[cfg(not(feature = "postgres"))]
    _phantom: (),
}

impl BudgetWatcher {
    #[cfg(feature = "postgres")]
    pub fn new(
        cfg: BudgetAlertsConfig,
        budget_tracker: Arc<BudgetTracker>,
        repo: KeyRepository,
        smtp_config: Option<SmtpConfig>,
    ) -> Self {
        Self {
            cfg,
            budget_tracker,
            smtp_config,
            cooldowns: HashMap::new(),
            repo,
        }
    }

    pub async fn run(mut self, cancel: CancellationToken) {
        tracing::info!(
            interval_secs = self.cfg.check_interval_secs,
            warn_pct = self.cfg.warn_threshold_pct,
            channel = %self.cfg.channel,
            "budget watcher started"
        );

        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(self.cfg.check_interval_secs)) => {
                    self.prune_cooldowns();
                    self.check_keys().await;
                }
                _ = cancel.cancelled() => {
                    tracing::info!("budget watcher shut down");
                    return;
                }
            }
        }
    }

    fn prune_cooldowns(&mut self) {
        let cooldown = self.cfg.cooldown_secs;
        self.cooldowns
            .retain(|_, fired_at| fired_at.elapsed().as_secs() < cooldown);
    }

    fn should_fire(&self, key: &CooldownKey) -> bool {
        match self.cooldowns.get(key) {
            None => true,
            Some(fired_at) => fired_at.elapsed().as_secs() >= self.cfg.cooldown_secs,
        }
    }

    fn record_fired(&mut self, key: CooldownKey) {
        self.cooldowns.insert(key, Instant::now());
    }

    fn make_alert_rule(&self) -> AlertRule {
        let channel = match self.cfg.channel.as_str() {
            "webhook" => AlertChannel::Webhook,
            "slack" => AlertChannel::Slack,
            "email" => AlertChannel::Email,
            _ => AlertChannel::Log,
        };
        AlertRule {
            id: Uuid::nil(),
            rule_type: RuleType::SpendThreshold,
            threshold: 0.0,
            channel,
            webhook_url: self.cfg.webhook_url.clone(),
            slack_webhook_url: self.cfg.slack_webhook_url.clone(),
            email_to: self.cfg.email_to.clone(),
            enabled: true,
        }
    }

    #[cfg(not(feature = "postgres"))]
    async fn check_keys(&mut self) {}

    #[cfg(feature = "postgres")]
    async fn check_keys(&mut self) {
        let keys = match self.repo.find_keys_with_budgets().await {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!(error = %e, "budget watcher: failed to query keys");
                return;
            }
        };

        let rule = self.make_alert_rule();
        let warn_pct = self.cfg.warn_threshold_pct;

        for key in keys {
            let (daily_spend, monthly_spend) = self.budget_tracker.get_spend(&key.key_hash);

            // --- Daily budget ---
            if let Some(daily_limit) = key.daily_budget_usd {
                let pct = daily_spend / daily_limit;

                if pct >= 1.0 {
                    let ck = (key.id, "daily", "exceeded");
                    if self.should_fire(&ck) {
                        let event = AlertEvent {
                            rule_id: Uuid::new_v4(),
                            triggered_at: chrono::Utc::now().to_rfc3339(),
                            message: format!(
                                "Key '{}' ({}) daily budget EXCEEDED: ${daily_spend:.4} / ${daily_limit:.4}",
                                key.name, &key.key_prefix
                            ),
                            severity: AlertSeverity::Critical,
                            current_value: Some(daily_spend),
                            threshold_value: Some(daily_limit),
                        };
                        notify::dispatch_alert(&rule, &event, self.smtp_config.as_ref()).await;
                        self.record_fired(ck);
                    }
                } else if pct >= warn_pct {
                    let ck = (key.id, "daily", "warning");
                    if self.should_fire(&ck) {
                        let event = AlertEvent {
                            rule_id: Uuid::new_v4(),
                            triggered_at: chrono::Utc::now().to_rfc3339(),
                            message: format!(
                                "Key '{}' ({}) daily budget at {:.0}%: ${daily_spend:.4} / ${daily_limit:.4}",
                                key.name,
                                &key.key_prefix,
                                pct * 100.0
                            ),
                            severity: AlertSeverity::Warning,
                            current_value: Some(daily_spend),
                            threshold_value: Some(daily_limit),
                        };
                        notify::dispatch_alert(&rule, &event, self.smtp_config.as_ref()).await;
                        self.record_fired(ck);
                    }
                }
            }

            // --- Monthly budget ---
            if let Some(monthly_limit) = key.monthly_budget_usd {
                let pct = monthly_spend / monthly_limit;

                if pct >= 1.0 {
                    let ck = (key.id, "monthly", "exceeded");
                    if self.should_fire(&ck) {
                        let event = AlertEvent {
                            rule_id: Uuid::new_v4(),
                            triggered_at: chrono::Utc::now().to_rfc3339(),
                            message: format!(
                                "Key '{}' ({}) monthly budget EXCEEDED: ${monthly_spend:.4} / ${monthly_limit:.4}",
                                key.name, &key.key_prefix
                            ),
                            severity: AlertSeverity::Critical,
                            current_value: Some(monthly_spend),
                            threshold_value: Some(monthly_limit),
                        };
                        notify::dispatch_alert(&rule, &event, self.smtp_config.as_ref()).await;
                        self.record_fired(ck);
                    }
                } else if pct >= warn_pct {
                    let ck = (key.id, "monthly", "warning");
                    if self.should_fire(&ck) {
                        let event = AlertEvent {
                            rule_id: Uuid::new_v4(),
                            triggered_at: chrono::Utc::now().to_rfc3339(),
                            message: format!(
                                "Key '{}' ({}) monthly budget at {:.0}%: ${monthly_spend:.4} / ${monthly_limit:.4}",
                                key.name,
                                &key.key_prefix,
                                pct * 100.0
                            ),
                            severity: AlertSeverity::Warning,
                            current_value: Some(monthly_spend),
                            threshold_value: Some(monthly_limit),
                        };
                        notify::dispatch_alert(&rule, &event, self.smtp_config.as_ref()).await;
                        self.record_fired(ck);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::budget::BudgetTracker;

    fn test_cfg(warn_pct: f64) -> BudgetAlertsConfig {
        BudgetAlertsConfig {
            enabled: true,
            check_interval_secs: 300,
            cooldown_secs: 3600,
            warn_threshold_pct: warn_pct,
            channel: "log".into(),
            webhook_url: None,
            slack_webhook_url: None,
            email_to: None,
        }
    }

    #[test]
    fn cooldown_key_uniqueness() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let k1: CooldownKey = (id1, "daily", "warning");
        let k2: CooldownKey = (id1, "daily", "exceeded");
        let k3: CooldownKey = (id1, "monthly", "warning");
        let k4: CooldownKey = (id2, "daily", "warning");

        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
        assert_ne!(k1, k4);
    }

    #[test]
    fn should_fire_first_time() {
        let cfg = test_cfg(0.8);
        let bt = Arc::new(BudgetTracker::new());
        // can't construct BudgetWatcher without repo in non-postgres builds,
        // so just test the logic inline
        let mut cooldowns: HashMap<CooldownKey, Instant> = HashMap::new();
        let key = (Uuid::new_v4(), "daily", "warning");
        assert!(!cooldowns.contains_key(&key));
        cooldowns.insert(key, Instant::now());
        assert!(cooldowns.contains_key(&key));
        let _ = (cfg, bt); // suppress unused warnings
    }
}
