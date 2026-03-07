use std::sync::Arc;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};

use crate::api::AppState;
use crate::error::Error;

#[derive(Clone, Debug)]
pub struct WorkspaceId(pub uuid::Uuid);

#[derive(Clone, Debug)]
pub struct AgentName(pub String);

pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, Error> {
    let raw_key = extract_key(req.headers()).ok_or(Error::Unauthorized)?;
    let hash = hash_key(&raw_key);
    let api_key = state
        .store
        .get_api_key_by_hash(&hash)
        .await
        .map_err(|_| Error::Unauthorized)?;
    req.extensions_mut()
        .insert(WorkspaceId(api_key.workspace_id));
    let agent_name = req
        .headers()
        .get("X-Agent-Name")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    if let Some(name) = agent_name {
        req.extensions_mut().insert(AgentName(name));
    }
    Ok(next.run(req).await)
}

fn extract_key(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(key) = headers.get("X-API-Key").and_then(|v| v.to_str().ok()) {
        if !key.is_empty() {
            return Some(key.to_string());
        }
    }
    if let Some(auth) = headers.get("Authorization").and_then(|v| v.to_str().ok()) {
        if let Some(token) = auth.strip_prefix("Bearer ") {
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

pub fn hash_key(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}
