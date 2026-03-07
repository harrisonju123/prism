use std::collections::HashMap;
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::SmtpConfig;

use super::anomaly;
use super::notify;
use super::types::{AlertEvent, AlertRule, AlertSeverity, RuleType};

/// Background alert checker that evaluates rules on a timer.
pub struct AlertChecker {
    rules: Vec<AlertRule>,
    ch_url: String,
    ch_db: String,
    check_interval_secs: u64,
    cooldown_secs: u64,
    cancel: CancellationToken,
    last_fired: HashMap<Uuid, Instant>,
    smtp_config: Option<SmtpConfig>,
}

impl AlertChecker {
    pub fn new(
        rules: Vec<AlertRule>,
        ch_url: String,
        ch_db: String,
        check_interval_secs: u64,
        cooldown_secs: u64,
        cancel: CancellationToken,
        smtp_config: Option<SmtpConfig>,
    ) -> Self {
        Self {
            rules,
            ch_url,
            ch_db,
            check_interval_secs,
            cooldown_secs,
            cancel,
            last_fired: HashMap::new(),
            smtp_config,
        }
    }

    pub async fn run(mut self) {
        tracing::info!(
            rules = self.rules.len(),
            interval_secs = self.check_interval_secs,
            "alert checker started"
        );

        loop {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(self.check_interval_secs)) => {
                    self.prune_fired();
                    if let Err(e) = self.evaluate_rules().await {
                        tracing::warn!(error = %e, "alert checker evaluation failed");
                    }
                }
                _ = self.cancel.cancelled() => {
                    tracing::info!("alert checker shut down");
                    return;
                }
            }
        }
    }

    fn should_fire(&self, rule_id: &Uuid) -> bool {
        match self.last_fired.get(rule_id) {
            None => true,
            Some(last) => last.elapsed().as_secs() >= self.cooldown_secs,
        }
    }

    fn record_fired(&mut self, rule_id: Uuid) {
        self.last_fired.insert(rule_id, Instant::now());
    }

    fn prune_fired(&mut self) {
        self.last_fired
            .retain(|_, v| v.elapsed().as_secs() < self.cooldown_secs);
    }

    async fn evaluate_rules(&mut self) -> anyhow::Result<()> {
        let client = reqwest::Client::new();

        for rule in &self.rules.clone() {
            if !rule.enabled || !self.should_fire(&rule.id) {
                continue;
            }

            let event = match rule.rule_type {
                RuleType::SpendThreshold => self.check_spend_threshold(&client, rule).await?,
                RuleType::AnomalyZscore => self.check_anomaly_zscore(&client, rule).await?,
                RuleType::ErrorRate => self.check_error_rate(&client, rule).await?,
                RuleType::LatencyP95 => self.check_latency_p95(&client, rule).await?,
            };

            if let Some(event) = event {
                self.record_fired(rule.id);
                notify::dispatch_alert(rule, &event, self.smtp_config.as_ref()).await;
            }
        }

        Ok(())
    }

    async fn check_spend_threshold(
        &self,
        client: &reqwest::Client,
        rule: &AlertRule,
    ) -> anyhow::Result<Option<AlertEvent>> {
        let query = format!(
            "SELECT sum(estimated_cost_usd) as today_spend \
             FROM {db}.inference_events \
             WHERE toDate(timestamp) = today() \
             FORMAT JSONEachRow",
            db = self.ch_db
        );

        let resp = client
            .post(&self.ch_url)
            .body(query)
            .send()
            .await?
            .text()
            .await?;
        let today_spend = parse_single_float(&resp, "today_spend");

        if today_spend > rule.threshold {
            Ok(Some(AlertEvent {
                rule_id: rule.id,
                triggered_at: chrono::Utc::now().to_rfc3339(),
                message: format!(
                    "Daily spend ${today_spend:.2} exceeds threshold ${:.2}",
                    rule.threshold
                ),
                severity: AlertSeverity::Warning,
                current_value: Some(today_spend),
                threshold_value: Some(rule.threshold),
            }))
        } else {
            Ok(None)
        }
    }

    async fn check_anomaly_zscore(
        &self,
        client: &reqwest::Client,
        rule: &AlertRule,
    ) -> anyhow::Result<Option<AlertEvent>> {
        // Get today's spend
        let today_query = format!(
            "SELECT sum(estimated_cost_usd) as today_spend \
             FROM {db}.inference_events \
             WHERE toDate(timestamp) = today() \
             FORMAT JSONEachRow",
            db = self.ch_db
        );

        let today_resp = client
            .post(&self.ch_url)
            .body(today_query)
            .send()
            .await?
            .text()
            .await?;
        let today_spend = parse_single_float(&today_resp, "today_spend");

        // Get 7-day baseline (mean, stddev)
        let baseline_query = format!(
            "SELECT avg(daily_cost) as mean_cost, stddevPop(daily_cost) as std_cost \
             FROM ( \
                SELECT toDate(timestamp) as day, sum(estimated_cost_usd) as daily_cost \
                FROM {db}.inference_events \
                WHERE timestamp >= now() - INTERVAL 8 DAY \
                  AND toDate(timestamp) < today() \
                GROUP BY day \
             ) \
             FORMAT JSONEachRow",
            db = self.ch_db
        );

        let baseline_resp = client
            .post(&self.ch_url)
            .body(baseline_query)
            .send()
            .await?
            .text()
            .await?;

        let (mean, stddev) = parse_mean_stddev(&baseline_resp);
        let warning_z = rule.threshold.max(2.0);
        let critical_z = (rule.threshold + 1.0).max(3.0);

        let result =
            anomaly::classify_spend_anomaly(today_spend, mean, stddev, warning_z, critical_z);

        if result.is_anomaly {
            Ok(Some(AlertEvent {
                rule_id: rule.id,
                triggered_at: chrono::Utc::now().to_rfc3339(),
                message: result.message,
                severity: result.severity,
                current_value: Some(today_spend),
                threshold_value: Some(mean),
            }))
        } else {
            Ok(None)
        }
    }

    async fn check_error_rate(
        &self,
        client: &reqwest::Client,
        rule: &AlertRule,
    ) -> anyhow::Result<Option<AlertEvent>> {
        let query = format!(
            "SELECT \
                countIf(status = 'failure') as failures, \
                count() as total \
             FROM {db}.inference_events \
             WHERE timestamp >= now() - INTERVAL 1 HOUR \
             FORMAT JSONEachRow",
            db = self.ch_db
        );

        let resp = client
            .post(&self.ch_url)
            .body(query)
            .send()
            .await?
            .text()
            .await?;

        let (failures, total) = parse_two_u64(&resp, "failures", "total");
        let error_rate = if total > 0 {
            failures as f64 / total as f64
        } else {
            0.0
        };

        if error_rate > rule.threshold {
            Ok(Some(AlertEvent {
                rule_id: rule.id,
                triggered_at: chrono::Utc::now().to_rfc3339(),
                message: format!(
                    "Error rate {:.1}% exceeds threshold {:.1}% ({failures}/{total} in last hour)",
                    error_rate * 100.0,
                    rule.threshold * 100.0
                ),
                severity: if error_rate > rule.threshold * 2.0 {
                    AlertSeverity::Critical
                } else {
                    AlertSeverity::Warning
                },
                current_value: Some(error_rate),
                threshold_value: Some(rule.threshold),
            }))
        } else {
            Ok(None)
        }
    }

    async fn check_latency_p95(
        &self,
        client: &reqwest::Client,
        rule: &AlertRule,
    ) -> anyhow::Result<Option<AlertEvent>> {
        let query = format!(
            "SELECT quantile(0.95)(latency_ms) as p95 \
             FROM {db}.inference_events \
             WHERE timestamp >= now() - INTERVAL 1 HOUR \
             FORMAT JSONEachRow",
            db = self.ch_db
        );

        let resp = client
            .post(&self.ch_url)
            .body(query)
            .send()
            .await?
            .text()
            .await?;
        let p95 = parse_single_float(&resp, "p95");

        if p95 > rule.threshold {
            Ok(Some(AlertEvent {
                rule_id: rule.id,
                triggered_at: chrono::Utc::now().to_rfc3339(),
                message: format!(
                    "P95 latency {p95:.0}ms exceeds threshold {:.0}ms",
                    rule.threshold
                ),
                severity: if p95 > rule.threshold * 2.0 {
                    AlertSeverity::Critical
                } else {
                    AlertSeverity::Warning
                },
                current_value: Some(p95),
                threshold_value: Some(rule.threshold),
            }))
        } else {
            Ok(None)
        }
    }
}

fn parse_single_float(resp: &str, field: &str) -> f64 {
    resp.lines()
        .find_map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| v.get(field).and_then(|v| v.as_f64()))
        })
        .unwrap_or(0.0)
}

fn parse_mean_stddev(resp: &str) -> (f64, f64) {
    resp.lines()
        .find_map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .map(|v| {
                    let mean = v.get("mean_cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let std = v.get("std_cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    (mean, std)
                })
        })
        .unwrap_or((0.0, 0.0))
}

fn parse_two_u64(resp: &str, field1: &str, field2: &str) -> (u64, u64) {
    resp.lines()
        .find_map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .map(|v| {
                    let a = v.get(field1).and_then(|v| v.as_u64()).unwrap_or(0);
                    let b = v.get(field2).and_then(|v| v.as_u64()).unwrap_or(0);
                    (a, b)
                })
        })
        .unwrap_or((0, 0))
}
