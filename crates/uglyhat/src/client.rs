use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::model::*;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct HttpClient {
    inner: reqwest::Client,
    base_url: String,
    api_key: String,
    workspace_id: Uuid,
}

impl HttpClient {
    pub fn new(base_url: String, api_key: String, workspace_id: Uuid) -> Self {
        Self {
            inner: reqwest::Client::new(),
            base_url,
            api_key,
            workspace_id,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    fn ws_url(&self, suffix: &str) -> String {
        self.url(&format!(
            "/workspaces/{}{suffix}",
            self.workspace_id
        ))
    }

    async fn get<T: DeserializeOwned>(&self, url: &str) -> Result<T, ClientError> {
        let resp = self
            .inner
            .get(url)
            .header("X-API-Key", &self.api_key)
            .send()
            .await?;
        handle_response(resp).await
    }

    async fn post<B: serde::Serialize, T: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let resp = self
            .inner
            .post(url)
            .header("X-API-Key", &self.api_key)
            .json(body)
            .send()
            .await?;
        handle_response(resp).await
    }

    async fn put<B: serde::Serialize, T: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let resp = self
            .inner
            .put(url)
            .header("X-API-Key", &self.api_key)
            .json(body)
            .send()
            .await?;
        handle_response(resp).await
    }

    // -------------------------------------------------------------------------
    // Context
    // -------------------------------------------------------------------------

    pub async fn get_context(&self) -> Result<WorkspaceContext, ClientError> {
        self.get(&self.ws_url("/context")).await
    }

    pub async fn get_next_tasks(&self, limit: i64) -> Result<Vec<TaskSummary>, ClientError> {
        self.get(&self.ws_url(&format!("/next?limit={limit}")))
            .await
    }

    // -------------------------------------------------------------------------
    // Tasks
    // -------------------------------------------------------------------------

    pub async fn get_task(&self, id: Uuid) -> Result<Task, ClientError> {
        self.get(&self.url(&format!("/tasks/{id}"))).await
    }

    pub async fn update_task(&self, id: Uuid, body: &Value) -> Result<Task, ClientError> {
        self.put(&self.url(&format!("/tasks/{id}")), body).await
    }

    pub async fn claim_task(&self, id: Uuid, agent_name: &str) -> Result<Task, ClientError> {
        let body = serde_json::json!({ "agent_name": agent_name });
        self.post(&self.url(&format!("/tasks/{id}/claim")), &body)
            .await
    }

    pub async fn list_tasks(
        &self,
        status: Option<&str>,
        domain: Option<&str>,
        assignee: Option<&str>,
        unassigned: bool,
    ) -> Result<Vec<Task>, ClientError> {
        let mut params = Vec::new();
        if let Some(s) = status {
            params.push(format!("status={s}"));
        }
        if let Some(d) = domain {
            params.push(format!("domain={d}"));
        }
        if let Some(a) = assignee {
            params.push(format!("assignee={a}"));
        }
        if unassigned {
            params.push("unassigned=true".to_string());
        }
        let qs = if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        };
        self.get(&self.ws_url(&format!("/tasks{qs}"))).await
    }

    pub async fn create_task(
        &self,
        epic_id: Uuid,
        name: &str,
        desc: &str,
        status: &str,
        priority: &str,
        assignee: &str,
        tags: Vec<String>,
    ) -> Result<Task, ClientError> {
        let body = serde_json::json!({
            "name": name,
            "description": desc,
            "status": status,
            "priority": priority,
            "assignee": assignee,
            "domain_tags": tags,
        });
        self.post(
            &self.url(&format!("/epics/{epic_id}/tasks")),
            &body,
        )
        .await
    }

    pub async fn get_task_deps(
        &self,
        id: Uuid,
    ) -> Result<Value, ClientError> {
        self.get(&self.url(&format!("/tasks/{id}/dependencies")))
            .await
    }

    pub async fn add_dependency(
        &self,
        blocking_id: Uuid,
        blocked_id: Uuid,
    ) -> Result<Value, ClientError> {
        let body = serde_json::json!({
            "blocking_task_id": blocking_id,
            "blocked_task_id": blocked_id,
        });
        self.post(
            &self.url(&format!("/tasks/{blocking_id}/dependencies")),
            &body,
        )
        .await
    }

    pub async fn get_task_context(&self, id: Uuid) -> Result<Value, ClientError> {
        self.get(&self.url(&format!("/tasks/{id}/context"))).await
    }

    pub async fn report_issue(&self, body: &Value) -> Result<Value, ClientError> {
        self.post(&self.ws_url("/issues"), body).await
    }

    // -------------------------------------------------------------------------
    // Initiatives
    // -------------------------------------------------------------------------

    pub async fn list_initiatives(&self) -> Result<Vec<Initiative>, ClientError> {
        self.get(&self.ws_url("/initiatives")).await
    }

    pub async fn create_initiative(
        &self,
        name: &str,
        desc: &str,
    ) -> Result<Initiative, ClientError> {
        let body = serde_json::json!({ "name": name, "description": desc });
        self.post(&self.ws_url("/initiatives"), &body).await
    }

    // -------------------------------------------------------------------------
    // Epics
    // -------------------------------------------------------------------------

    pub async fn list_epics(&self, initiative_id: Uuid) -> Result<Vec<Epic>, ClientError> {
        self.get(&self.url(&format!("/initiatives/{initiative_id}/epics")))
            .await
    }

    pub async fn create_epic(
        &self,
        initiative_id: Uuid,
        name: &str,
        desc: &str,
    ) -> Result<Epic, ClientError> {
        let body = serde_json::json!({ "name": name, "description": desc });
        self.post(
            &self.url(&format!("/initiatives/{initiative_id}/epics")),
            &body,
        )
        .await
    }

    // -------------------------------------------------------------------------
    // Agents
    // -------------------------------------------------------------------------

    pub async fn checkin(
        &self,
        name: &str,
        capabilities: Vec<String>,
    ) -> Result<CheckinResponse, ClientError> {
        let body = serde_json::json!({ "name": name, "capabilities": capabilities });
        self.post(&self.ws_url("/agents/checkin"), &body).await
    }

    pub async fn checkout(
        &self,
        name: &str,
        summary: &str,
    ) -> Result<AgentSession, ClientError> {
        let body = serde_json::json!({ "name": name, "summary": summary });
        self.post(&self.ws_url("/agents/checkout"), &body).await
    }

    pub async fn list_agent_statuses(&self) -> Result<Vec<AgentStatus>, ClientError> {
        self.get(&self.ws_url("/agents/statuses")).await
    }

    // -------------------------------------------------------------------------
    // Handoffs
    // -------------------------------------------------------------------------

    pub async fn create_handoff(&self, body: &Value) -> Result<Value, ClientError> {
        self.post(&self.ws_url("/handoffs"), body).await
    }

    // -------------------------------------------------------------------------
    // Decisions
    // -------------------------------------------------------------------------

    pub async fn list_decisions(&self) -> Result<Vec<Decision>, ClientError> {
        self.get(&self.ws_url("/decisions")).await
    }

    pub async fn create_decision(&self, body: &Value) -> Result<Decision, ClientError> {
        self.post(&self.url("/decisions"), body).await
    }

    // -------------------------------------------------------------------------
    // Notes
    // -------------------------------------------------------------------------

    pub async fn create_note(&self, body: &Value) -> Result<Note, ClientError> {
        self.post(&self.url("/notes"), body).await
    }

    // -------------------------------------------------------------------------
    // Activity
    // -------------------------------------------------------------------------

    pub async fn list_activity(
        &self,
        since: Option<&str>,
        actor: Option<&str>,
        limit: i64,
    ) -> Result<Vec<ActivityEntry>, ClientError> {
        let mut params = vec![format!("limit={limit}")];
        if let Some(s) = since {
            params.push(format!("since={s}"));
        }
        if let Some(a) = actor {
            params.push(format!("actor={a}"));
        }
        let qs = format!("?{}", params.join("&"));
        self.get(&self.ws_url(&format!("/activity{qs}"))).await
    }
}

async fn handle_response<T: DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, ClientError> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp.json::<T>().await?)
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(ClientError::Http {
            status: status.as_u16(),
            body,
        })
    }
}
