use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;

use super::frameworks::{ComplianceSection, ComplianceStatus, Framework};

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    pub framework: String,
    #[serde(default = "default_days")]
    pub days: u32,
}

fn default_days() -> u32 {
    30
}

#[derive(Debug, Serialize)]
pub struct ComplianceReport {
    pub framework: Framework,
    pub period_days: u32,
    pub generated_at: String,
    pub sections: Vec<ComplianceSection>,
    pub summary: ComplianceSummary,
}

#[derive(Debug, Serialize)]
pub struct ComplianceSummary {
    pub total_sections: usize,
    pub compliant: usize,
    pub partial: usize,
    pub non_compliant: usize,
    pub total_events_audited: u64,
}

/// GET /api/v1/compliance/export?framework=soc2&days=30
pub async fn export_compliance(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExportQuery>,
) -> Result<Response> {
    let framework = Framework::from_str(&params.framework).ok_or_else(|| {
        PrismError::BadRequest(format!(
            "unknown framework '{}', supported: soc2, iso27001, hipaa",
            params.framework
        ))
    })?;

    let client = reqwest::Client::new();

    // Query audit event counts from ClickHouse
    let total_events = query_event_count(
        &client,
        &state.config.clickhouse.url,
        &state.config.clickhouse.database,
        params.days,
    )
    .await
    .unwrap_or(0);

    let access_event_count = query_access_events(
        &client,
        &state.config.clickhouse.url,
        &state.config.clickhouse.database,
        params.days,
    )
    .await
    .unwrap_or(0);

    // Build sections based on framework requirements
    let mut sections = Vec::new();
    for &section_name in framework.required_sections() {
        let (status, details, evidence) = match section_name {
            "access_controls" => {
                if access_event_count > 0 {
                    (
                        ComplianceStatus::Compliant,
                        format!("{access_event_count} access events logged with virtual key auth"),
                        access_event_count,
                    )
                } else {
                    (
                        ComplianceStatus::Partial,
                        "no access events found — virtual keys may not be enabled".into(),
                        0,
                    )
                }
            }
            "audit_logging" => {
                if total_events > 0 {
                    (
                        ComplianceStatus::Compliant,
                        format!("{total_events} inference events logged to ClickHouse"),
                        total_events,
                    )
                } else {
                    (
                        ComplianceStatus::NonCompliant,
                        "no audit events found".into(),
                        0,
                    )
                }
            }
            "data_encryption" => (
                ComplianceStatus::Compliant,
                "prompt content is hashed (SHA-256), never stored in plaintext".into(),
                total_events,
            ),
            _ => (
                ComplianceStatus::NotApplicable,
                format!("section '{section_name}' requires manual review"),
                0,
            ),
        };

        sections.push(ComplianceSection {
            name: section_name.to_string(),
            status,
            details,
            evidence_count: evidence,
        });
    }

    let compliant = sections
        .iter()
        .filter(|s| matches!(s.status, ComplianceStatus::Compliant))
        .count();
    let partial = sections
        .iter()
        .filter(|s| matches!(s.status, ComplianceStatus::Partial))
        .count();
    let non_compliant = sections
        .iter()
        .filter(|s| matches!(s.status, ComplianceStatus::NonCompliant))
        .count();

    let report = ComplianceReport {
        framework,
        period_days: params.days,
        generated_at: chrono::Utc::now().to_rfc3339(),
        summary: ComplianceSummary {
            total_sections: sections.len(),
            compliant,
            partial,
            non_compliant,
            total_events_audited: total_events,
        },
        sections,
    };

    Ok(Json(report).into_response())
}

async fn query_event_count(
    client: &reqwest::Client,
    ch_url: &str,
    ch_db: &str,
    days: u32,
) -> anyhow::Result<u64> {
    let query = format!(
        "SELECT count() as cnt FROM {db}.inference_events \
         WHERE timestamp >= now() - INTERVAL {days} DAY \
         FORMAT JSONEachRow",
        db = ch_db,
    );
    let resp = client.post(ch_url).body(query).send().await?.text().await?;
    for line in resp.lines() {
        if let Ok(row) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(cnt) = row.get("cnt").and_then(|v| v.as_u64()) {
                return Ok(cnt);
            }
        }
    }
    Ok(0)
}

async fn query_access_events(
    client: &reqwest::Client,
    ch_url: &str,
    ch_db: &str,
    days: u32,
) -> anyhow::Result<u64> {
    let query = format!(
        "SELECT count() as cnt FROM {db}.inference_events \
         WHERE timestamp >= now() - INTERVAL {days} DAY \
         AND virtual_key_hash IS NOT NULL AND virtual_key_hash != '' \
         FORMAT JSONEachRow",
        db = ch_db,
    );
    let resp = client.post(ch_url).body(query).send().await?.text().await?;
    for line in resp.lines() {
        if let Ok(row) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(cnt) = row.get("cnt").and_then(|v| v.as_u64()) {
                return Ok(cnt);
            }
        }
    }
    Ok(0)
}
