use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{PrismError, Result};
use crate::keys::MasterAuth;
use crate::proxy::handler::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateBudgetNodeRequest {
    pub parent_id: Option<Uuid>,
    pub node_type: String,
    pub node_id: String,
    pub daily_budget_usd: Option<f64>,
    pub monthly_budget_usd: Option<f64>,
    #[serde(default = "default_budget_action")]
    pub budget_action: String,
}

fn default_budget_action() -> String {
    "reject".into()
}

#[derive(Debug, Deserialize)]
pub struct UpdateBudgetNodeRequest {
    pub daily_budget_usd: Option<Option<f64>>,
    pub monthly_budget_usd: Option<Option<f64>>,
    pub budget_action: Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct BudgetNodeResponse {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub node_type: String,
    pub node_id: String,
    pub daily_budget_usd: Option<f64>,
    pub monthly_budget_usd: Option<f64>,
    pub budget_action: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

fn get_pool(state: &crate::proxy::handler::AppState) -> Result<&sqlx::PgPool> {
    state
        .key_service
        .as_ref()
        .map(|ks| ks.repo().pool())
        .ok_or_else(|| PrismError::Internal("postgres not available".into()))
}

/// GET /api/v1/budgets/hierarchy
pub async fn get_hierarchy(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
) -> Result<Json<Vec<BudgetNodeResponse>>> {
    let pool = get_pool(&state)?;
    let nodes = sqlx::query_as::<_, BudgetNodeResponse>(
        "SELECT id, parent_id, node_type, node_id, daily_budget_usd, monthly_budget_usd, budget_action, created_at, updated_at FROM budget_nodes ORDER BY created_at",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| PrismError::Internal(format!("failed to fetch hierarchy: {e}")))?;

    Ok(Json(nodes))
}

/// POST /api/v1/budgets/nodes
pub async fn create_node(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Json(body): Json<CreateBudgetNodeRequest>,
) -> Result<Json<BudgetNodeResponse>> {
    let pool = get_pool(&state)?;
    let node = sqlx::query_as::<_, BudgetNodeResponse>(
        r#"INSERT INTO budget_nodes (parent_id, node_type, node_id, daily_budget_usd, monthly_budget_usd, budget_action)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, parent_id, node_type, node_id, daily_budget_usd, monthly_budget_usd, budget_action, created_at, updated_at"#,
    )
    .bind(body.parent_id)
    .bind(&body.node_type)
    .bind(&body.node_id)
    .bind(body.daily_budget_usd)
    .bind(body.monthly_budget_usd)
    .bind(&body.budget_action)
    .fetch_one(pool)
    .await
    .map_err(|e| PrismError::Internal(format!("failed to create budget node: {e}")))?;

    Ok(Json(node))
}

/// PUT /api/v1/budgets/nodes/:id
pub async fn update_node(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateBudgetNodeRequest>,
) -> Result<Json<BudgetNodeResponse>> {
    let pool = get_pool(&state)?;
    let node = sqlx::query_as::<_, BudgetNodeResponse>(
        r#"UPDATE budget_nodes SET
            daily_budget_usd = CASE WHEN $2::boolean THEN $3 ELSE daily_budget_usd END,
            monthly_budget_usd = CASE WHEN $4::boolean THEN $5 ELSE monthly_budget_usd END,
            budget_action = COALESCE($6, budget_action),
            updated_at = NOW()
           WHERE id = $1
           RETURNING id, parent_id, node_type, node_id, daily_budget_usd, monthly_budget_usd, budget_action, created_at, updated_at"#,
    )
    .bind(id)
    .bind(body.daily_budget_usd.is_some())
    .bind(body.daily_budget_usd.flatten())
    .bind(body.monthly_budget_usd.is_some())
    .bind(body.monthly_budget_usd.flatten())
    .bind(body.budget_action)
    .fetch_optional(pool)
    .await
    .map_err(|e| PrismError::Internal(format!("failed to update budget node: {e}")))?
    .ok_or_else(|| PrismError::ModelNotFound(format!("budget node {id} not found")))?;

    Ok(Json(node))
}

/// DELETE /api/v1/budgets/nodes/:id
pub async fn delete_node(
    State(state): State<Arc<AppState>>,
    _auth: MasterAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let pool = get_pool(&state)?;
    let result = sqlx::query("DELETE FROM budget_nodes WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| PrismError::Internal(format!("failed to delete budget node: {e}")))?;

    if result.rows_affected() == 0 {
        return Err(PrismError::ModelNotFound(format!(
            "budget node {id} not found"
        )));
    }

    Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
}
