use super::types::{AlertChannel, AlertEvent, AlertRule, AlertSeverity};
use crate::config::SmtpConfig;

fn severity_emoji(severity: &AlertSeverity) -> &'static str {
    match severity {
        AlertSeverity::Info => ":information_source:",
        AlertSeverity::Warning => ":warning:",
        AlertSeverity::Critical => ":rotating_light:",
    }
}

fn format_slack_blocks(rule: &AlertRule, event: &AlertEvent) -> serde_json::Value {
    let emoji = severity_emoji(&event.severity);
    let severity_label = format!("{:?}", event.severity).to_uppercase();

    let header_text = format!(
        "{severity_label}: {rule_type:?}",
        rule_type = rule.rule_type
    );
    let section_text = format!(
        "{emoji} *{severity_label}* | {rule_type:?}",
        rule_type = rule.rule_type,
    );

    let mut body_lines = vec![event.message.clone()];
    if let Some(current) = event.current_value {
        body_lines.push(format!("*Current:* {current:.4}"));
    }
    if let Some(threshold) = event.threshold_value {
        body_lines.push(format!("*Threshold:* {threshold:.4}"));
    }

    let blocks = serde_json::json!([
        {
            "type": "header",
            "text": { "type": "plain_text", "text": header_text }
        },
        {
            "type": "section",
            "text": { "type": "mrkdwn", "text": section_text }
        },
        {
            "type": "section",
            "text": { "type": "mrkdwn", "text": body_lines.join("\n") }
        }
    ]);

    blocks
}

async fn send_slack_webhook(
    url: &str,
    event: &AlertEvent,
    blocks: serde_json::Value,
) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "text": event.message,
        "blocks": blocks,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("slack webhook returned status {}", resp.status());
    }

    tracing::debug!(url, "slack alert dispatched");
    Ok(())
}

fn format_email_body(rule: &AlertRule, event: &AlertEvent) -> String {
    let mut lines = vec![
        format!("PrisM Alert: {:?}", event.severity),
        format!("Rule: {:?}", rule.rule_type),
        String::new(),
        event.message.clone(),
        String::new(),
    ];
    if let Some(current) = event.current_value {
        lines.push(format!("Current value: {current:.4}"));
    }
    if let Some(threshold) = event.threshold_value {
        lines.push(format!("Threshold: {threshold:.4}"));
    }
    lines.extend_from_slice(&[
        String::new(),
        "---".to_string(),
        "View details in the PrisM dashboard.".to_string(),
    ]);
    lines.join("\n")
}

#[cfg(feature = "smtp")]
fn send_email_blocking(
    to: &str,
    subject: &str,
    body: &str,
    smtp_config: &SmtpConfig,
) -> anyhow::Result<()> {
    use lettre::{SmtpTransport, Transport, message::Message, transport::smtp::client::Tls};

    let email = Message::builder()
        .from(smtp_config.from_address.parse()?)
        .to(to.parse()?)
        .subject(subject)
        .body(body.to_string())?;

    let transport = SmtpTransport::builder_dangerous(&smtp_config.host)
        .port(smtp_config.port)
        .tls(Tls::None)
        .build();

    transport.send(&email)?;
    Ok(())
}

#[cfg(feature = "smtp")]
async fn send_email(
    to: &str,
    rule: &AlertRule,
    event: &AlertEvent,
    smtp_config: &SmtpConfig,
) -> anyhow::Result<()> {
    let subject = format!("[PrisM] {:?} alert - {:?}", rule.rule_type, event.severity);
    let body = format_email_body(rule, event);
    let to = to.to_string();
    let smtp_config = smtp_config.clone();

    tokio::task::spawn_blocking(move || send_email_blocking(&to, &subject, &body, &smtp_config))
        .await??;

    tracing::debug!("email alert dispatched");
    Ok(())
}

/// Send alert notification via the rule's configured channel.
pub async fn dispatch_alert(
    rule: &AlertRule,
    event: &AlertEvent,
    smtp_config: Option<&SmtpConfig>,
) {
    match rule.channel {
        AlertChannel::Webhook => {
            if let Some(ref url) = rule.webhook_url {
                if let Err(e) = send_webhook(url, event).await {
                    tracing::warn!(
                        rule_id = %rule.id,
                        error = %e,
                        "webhook dispatch failed"
                    );
                }
            } else {
                tracing::warn!(rule_id = %rule.id, "webhook channel but no URL configured");
            }
        }
        AlertChannel::Log => {
            tracing::warn!(
                rule_id = %rule.id,
                severity = ?event.severity,
                message = %event.message,
                "alert fired"
            );
        }
        AlertChannel::Slack => {
            if let Some(ref url) = rule.slack_webhook_url {
                let blocks = format_slack_blocks(rule, event);
                if let Err(e) = send_slack_webhook(url, event, blocks).await {
                    tracing::warn!(
                        rule_id = %rule.id,
                        error = %e,
                        "slack dispatch failed"
                    );
                }
            } else {
                tracing::warn!(rule_id = %rule.id, "slack channel but no webhook URL configured");
            }
        }
        AlertChannel::Email => {
            #[cfg(feature = "smtp")]
            {
                if let Some(ref to) = rule.email_to {
                    if let Some(smtp) = smtp_config {
                        if let Err(e) = send_email(to, rule, event, smtp).await {
                            tracing::warn!(
                                rule_id = %rule.id,
                                error = %e,
                                "email dispatch failed"
                            );
                        }
                    } else {
                        tracing::warn!(rule_id = %rule.id, "email channel but no SMTP config");
                    }
                } else {
                    tracing::warn!(rule_id = %rule.id, "email channel but no recipient configured");
                }
            }
            #[cfg(not(feature = "smtp"))]
            tracing::warn!(rule_id = %rule.id, "email channel but smtp feature not enabled");
        }
    }
}

async fn send_webhook(url: &str, event: &AlertEvent) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "text": event.message,
        "severity": event.severity,
        "rule_id": event.rule_id,
        "triggered_at": event.triggered_at,
        "current_value": event.current_value,
        "threshold_value": event.threshold_value,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("webhook returned status {}", resp.status());
    }

    tracing::debug!(url, "webhook alert dispatched");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_rule(channel: AlertChannel) -> AlertRule {
        AlertRule {
            id: Uuid::new_v4(),
            rule_type: super::super::types::RuleType::SpendThreshold,
            threshold: 100.0,
            channel,
            webhook_url: Some("https://example.com/webhook".into()),
            slack_webhook_url: Some("https://hooks.slack.com/test".into()),
            email_to: Some("admin@example.com".into()),
            enabled: true,
        }
    }

    fn test_event() -> AlertEvent {
        AlertEvent {
            rule_id: Uuid::new_v4(),
            triggered_at: "2026-03-06T00:00:00Z".into(),
            message: "Daily spend $150.00 exceeds threshold $100.00".into(),
            severity: AlertSeverity::Warning,
            current_value: Some(150.0),
            threshold_value: Some(100.0),
        }
    }

    #[test]
    fn test_severity_emoji() {
        assert_eq!(severity_emoji(&AlertSeverity::Info), ":information_source:");
        assert_eq!(severity_emoji(&AlertSeverity::Warning), ":warning:");
        assert_eq!(severity_emoji(&AlertSeverity::Critical), ":rotating_light:");
    }

    #[test]
    fn test_format_slack_blocks_structure() {
        let rule = test_rule(AlertChannel::Slack);
        let event = test_event();

        let blocks = format_slack_blocks(&rule, &event);
        let blocks_arr = blocks.as_array().expect("blocks should be an array");

        assert_eq!(blocks_arr.len(), 3);
        assert_eq!(blocks_arr[0]["type"], "header");
        assert_eq!(blocks_arr[1]["type"], "section");
        assert_eq!(blocks_arr[2]["type"], "section");

        // Header should contain severity and rule type
        let header = blocks_arr[0]["text"]["text"].as_str().unwrap();
        assert!(header.contains("WARNING"));
        assert!(header.contains("SpendThreshold"));

        // Second section should have emoji
        let section = blocks_arr[1]["text"]["text"].as_str().unwrap();
        assert!(section.contains(":warning:"));

        // Body should contain message and values
        let body = blocks_arr[2]["text"]["text"].as_str().unwrap();
        assert!(body.contains("Daily spend"));
        assert!(body.contains("*Current:*"));
        assert!(body.contains("*Threshold:*"));
    }

    #[test]
    fn test_format_slack_blocks_critical() {
        let rule = test_rule(AlertChannel::Slack);
        let event = AlertEvent {
            severity: AlertSeverity::Critical,
            ..test_event()
        };

        let blocks = format_slack_blocks(&rule, &event);
        let section = blocks[1]["text"]["text"].as_str().unwrap();
        assert!(section.contains(":rotating_light:"));
        assert!(section.contains("CRITICAL"));
    }

    #[test]
    fn test_format_slack_blocks_no_values() {
        let rule = test_rule(AlertChannel::Slack);
        let event = AlertEvent {
            current_value: None,
            threshold_value: None,
            ..test_event()
        };

        let blocks = format_slack_blocks(&rule, &event);
        let body = blocks[2]["text"]["text"].as_str().unwrap();
        assert!(!body.contains("*Current:*"));
        assert!(!body.contains("*Threshold:*"));
    }

    #[test]
    fn test_format_email_body() {
        let rule = test_rule(AlertChannel::Email);
        let event = test_event();

        let body = format_email_body(&rule, &event);
        assert!(body.contains("PrisM Alert: Warning"));
        assert!(body.contains("Rule: SpendThreshold"));
        assert!(body.contains("Daily spend"));
        assert!(body.contains("Current value:"));
        assert!(body.contains("Threshold:"));
        assert!(body.contains("PrisM dashboard"));
    }

    #[test]
    fn test_format_email_body_no_values() {
        let rule = test_rule(AlertChannel::Email);
        let event = AlertEvent {
            current_value: None,
            threshold_value: None,
            ..test_event()
        };

        let body = format_email_body(&rule, &event);
        assert!(!body.contains("Current value:"));
        assert!(!body.contains("Threshold:"));
    }

    #[tokio::test]
    async fn test_dispatch_log_channel() {
        let rule = test_rule(AlertChannel::Log);
        let event = test_event();
        // Should not panic — just logs
        dispatch_alert(&rule, &event, None).await;
    }

    #[tokio::test]
    async fn test_dispatch_email_no_smtp_config() {
        let rule = test_rule(AlertChannel::Email);
        let event = test_event();
        // Should warn about missing SMTP config, not panic
        dispatch_alert(&rule, &event, None).await;
    }

    #[tokio::test]
    async fn test_dispatch_slack_no_url() {
        let mut rule = test_rule(AlertChannel::Slack);
        rule.slack_webhook_url = None;
        let event = test_event();
        // Should warn about missing URL, not panic
        dispatch_alert(&rule, &event, None).await;
    }
}
