use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{PrismError, Result};

use crate::proxy::handler::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub title: String,
    #[serde(default)]
    pub symptom: serde_json::Value,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct CreateHypothesisRequest {
    pub rank: Option<i32>,
    pub statement: String,
    pub confidence: Option<f64>,
    #[serde(default)]
    pub evidence: serde_json::Value,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateExperimentRequest {
    pub hypothesis_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub cost_level: Option<String>,
    pub impact_level: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct CreateRunRequest {
    pub status: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i32>,
    pub output: Option<String>,
    #[serde(default)]
    pub artifacts: serde_json::Value,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DebugSessionSummary {
    pub id: Uuid,
    pub title: String,
    pub status: String,
    pub symptom: serde_json::Value,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DebugHypothesis {
    pub id: Uuid,
    pub session_id: Uuid,
    pub rank: i32,
    pub statement: String,
    pub confidence: f64,
    pub evidence: serde_json::Value,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DebugExperiment {
    pub id: Uuid,
    pub session_id: Uuid,
    pub hypothesis_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub cost_level: String,
    pub impact_level: String,
    pub status: String,
    pub params: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DebugRun {
    pub id: Uuid,
    pub experiment_id: Uuid,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i32>,
    pub output: Option<String>,
    pub artifacts: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct DebugSessionDetail {
    pub session: DebugSessionSummary,
    pub hypotheses: Vec<DebugHypothesis>,
    pub experiments: Vec<DebugExperiment>,
    pub runs: Vec<DebugRun>,
}

fn get_pool(state: &AppState) -> Result<&sqlx::PgPool> {
    state
        .pg_pool
        .as_ref()
        .ok_or_else(|| PrismError::Internal("postgres not available".into()))
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<DebugSessionSummary>>> {
    let pool = get_pool(&state)?;

    let rows = sqlx::query_as::<_, DebugSessionSummary>(
        r#"SELECT id, title, status, symptom, metadata, created_at, updated_at
           FROM debug_sessions
           ORDER BY created_at DESC
           LIMIT 200"#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| PrismError::Internal(format!("debug session query failed: {e}")))?;

    Ok(Json(rows))
}

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSessionRequest>,
) -> Result<Json<DebugSessionSummary>> {
    let pool = get_pool(&state)?;

    let row = sqlx::query_as::<_, DebugSessionSummary>(
        r#"INSERT INTO debug_sessions (title, symptom, metadata)
           VALUES ($1, $2, $3)
           RETURNING id, title, status, symptom, metadata, created_at, updated_at"#,
    )
    .bind(body.title)
    .bind(body.symptom)
    .bind(body.metadata)
    .fetch_one(pool)
    .await
    .map_err(|e| PrismError::Internal(format!("debug session insert failed: {e}")))?;

    Ok(Json(row))
}

pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<DebugSessionDetail>> {
    let pool = get_pool(&state)?;

    let session = sqlx::query_as::<_, DebugSessionSummary>(
        r#"SELECT id, title, status, symptom, metadata, created_at, updated_at
           FROM debug_sessions
           WHERE id = $1"#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| PrismError::Internal(format!("debug session query failed: {e}")))?;

    let session = session
        .ok_or_else(|| PrismError::NotFound(format!("debug session {session_id} not found")))?;

    // Fetch hypotheses, experiments, and runs in parallel — they're independent queries.
    let (hypotheses_res, experiments_res, runs_res) = tokio::join!(
        sqlx::query_as::<_, DebugHypothesis>(
            r#"SELECT id, session_id, rank, statement, confidence, evidence, status, created_at, updated_at
               FROM debug_hypotheses
               WHERE session_id = $1
               ORDER BY rank ASC, created_at ASC"#,
        )
        .bind(session_id)
        .fetch_all(pool),
        sqlx::query_as::<_, DebugExperiment>(
            r#"SELECT id, session_id, hypothesis_id, title, description, cost_level, impact_level, status, params, created_at, updated_at
               FROM debug_experiments
               WHERE session_id = $1
               ORDER BY created_at ASC"#,
        )
        .bind(session_id)
        .fetch_all(pool),
        sqlx::query_as::<_, DebugRun>(
            r#"SELECT id, experiment_id, status, started_at, finished_at, duration_ms, output, artifacts, created_at
               FROM debug_runs
               WHERE experiment_id IN (SELECT id FROM debug_experiments WHERE session_id = $1)
               ORDER BY created_at ASC"#,
        )
        .bind(session_id)
        .fetch_all(pool),
    );

    let hypotheses = hypotheses_res
        .map_err(|e| PrismError::Internal(format!("debug hypothesis query failed: {e}")))?;
    let experiments = experiments_res
        .map_err(|e| PrismError::Internal(format!("debug experiment query failed: {e}")))?;
    let runs =
        runs_res.map_err(|e| PrismError::Internal(format!("debug run query failed: {e}")))?;

    Ok(Json(DebugSessionDetail {
        session,
        hypotheses,
        experiments,
        runs,
    }))
}

pub async fn create_hypothesis(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<Uuid>,
    Json(body): Json<CreateHypothesisRequest>,
) -> Result<Json<DebugHypothesis>> {
    let pool = get_pool(&state)?;

    let rank = body.rank.unwrap_or(0);
    let confidence = body.confidence.unwrap_or(0.0);

    if !(0.0..=1.0).contains(&confidence) {
        return Err(PrismError::BadRequest(
            "confidence must be between 0.0 and 1.0".into(),
        ));
    }

    let status = body.status.unwrap_or_else(|| "active".to_string());
    if !["active", "confirmed", "rejected"].contains(&status.as_str()) {
        return Err(PrismError::BadRequest(format!(
            "invalid hypothesis status: {status}"
        )));
    }

    let row = sqlx::query_as::<_, DebugHypothesis>(
        r#"INSERT INTO debug_hypotheses (session_id, rank, statement, confidence, evidence, status)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, session_id, rank, statement, confidence, evidence, status, created_at, updated_at"#,
    )
    .bind(session_id)
    .bind(rank)
    .bind(body.statement)
    .bind(confidence)
    .bind(body.evidence)
    .bind(status)
    .fetch_one(pool)
    .await
    .map_err(|e| PrismError::Internal(format!("debug hypothesis insert failed: {e}")))?;

    Ok(Json(row))
}

pub async fn create_experiment(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<Uuid>,
    Json(body): Json<CreateExperimentRequest>,
) -> Result<Json<DebugExperiment>> {
    let pool = get_pool(&state)?;

    let cost_level = body.cost_level.unwrap_or_else(|| "medium".to_string());
    if !["low", "medium", "high"].contains(&cost_level.as_str()) {
        return Err(PrismError::BadRequest(format!(
            "invalid cost_level: {cost_level}"
        )));
    }

    let impact_level = body.impact_level.unwrap_or_else(|| "medium".to_string());
    if !["low", "medium", "high"].contains(&impact_level.as_str()) {
        return Err(PrismError::BadRequest(format!(
            "invalid impact_level: {impact_level}"
        )));
    }

    let status = body.status.unwrap_or_else(|| "proposed".to_string());
    if !["proposed", "running", "completed", "cancelled"].contains(&status.as_str()) {
        return Err(PrismError::BadRequest(format!(
            "invalid experiment status: {status}"
        )));
    }

    let row = sqlx::query_as::<_, DebugExperiment>(
        r#"INSERT INTO debug_experiments (session_id, hypothesis_id, title, description, cost_level, impact_level, status, params)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
           RETURNING id, session_id, hypothesis_id, title, description, cost_level, impact_level, status, params, created_at, updated_at"#,
    )
    .bind(session_id)
    .bind(body.hypothesis_id)
    .bind(body.title)
    .bind(body.description)
    .bind(cost_level)
    .bind(impact_level)
    .bind(status)
    .bind(body.params)
    .fetch_one(pool)
    .await
    .map_err(|e| PrismError::Internal(format!("debug experiment insert failed: {e}")))?;

    Ok(Json(row))
}

pub async fn create_run(
    State(state): State<Arc<AppState>>,
    Path(experiment_id): Path<Uuid>,
    Json(body): Json<CreateRunRequest>,
) -> Result<Json<DebugRun>> {
    let pool = get_pool(&state)?;

    let status = body.status.unwrap_or_else(|| "queued".to_string());
    if !["queued", "running", "completed", "failed"].contains(&status.as_str()) {
        return Err(PrismError::BadRequest(format!(
            "invalid run status: {status}"
        )));
    }

    let row = sqlx::query_as::<_, DebugRun>(
        r#"INSERT INTO debug_runs (experiment_id, status, started_at, finished_at, duration_ms, output, artifacts)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           RETURNING id, experiment_id, status, started_at, finished_at, duration_ms, output, artifacts, created_at"#,
    )
    .bind(experiment_id)
    .bind(status)
    .bind(body.started_at)
    .bind(body.finished_at)
    .bind(body.duration_ms)
    .bind(body.output)
    .bind(body.artifacts)
    .fetch_one(pool)
    .await
    .map_err(|e| PrismError::Internal(format!("debug run insert failed: {e}")))?;

    Ok(Json(row))
}
