use anyhow::{Result, anyhow};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct GithubIssue {
    pub number: i64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub html_url: String,
}

#[derive(Debug, Clone)]
pub struct GithubSync {
    pat: String,
    repo: String,
    base_url: String,
    client: reqwest::Client,
}

impl GithubSync {
    pub fn new(pat: impl Into<String>, repo: impl Into<String>) -> Self {
        Self::with_base_url(pat, repo, "https://api.github.com")
    }

    pub fn with_base_url(
        pat: impl Into<String>,
        repo: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            pat: pat.into(),
            repo: repo.into(),
            base_url: base_url.into(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn pull_issues(&self, since: Option<&str>) -> Result<Vec<GithubIssue>> {
        let mut url = format!(
            "{}/repos/{}/issues?state=open&per_page=100",
            self.base_url, self.repo
        );
        if let Some(since) = since {
            url.push_str(&format!("&since={since}"));
        }

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("token {}", self.pat))
            .header("User-Agent", "uglyhat/0.1")
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
            .map_err(|e| anyhow!("github API request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("github API error {status}: {body}"));
        }

        let issues: Vec<GithubIssue> = resp
            .json()
            .await
            .map_err(|e| anyhow!("github API parse error: {e}"))?;
        Ok(issues)
    }

    pub async fn push_comment(&self, issue_number: i64, comment: &str) -> Result<()> {
        let url = format!(
            "{}/repos/{}/issues/{}/comments",
            self.base_url, self.repo, issue_number
        );
        let body = serde_json::json!({ "body": comment });
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("token {}", self.pat))
            .header("User-Agent", "uglyhat/0.1")
            .header("Accept", "application/vnd.github.v3+json")
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("github comment failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            tracing::warn!(issue = issue_number, %status, "failed to post github comment");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn pull_issues_returns_parsed_issues() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/test/repo/issues"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {
                    "number": 42,
                    "title": "Fix the bug",
                    "body": "Description here",
                    "state": "open",
                    "html_url": "https://github.com/test/repo/issues/42"
                }
            ])))
            .expect(1)
            .mount(&mock_server)
            .await;

        let sync = GithubSync::with_base_url("test-pat", "test/repo", mock_server.uri());
        let issues = sync.pull_issues(None).await.unwrap();

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 42);
        assert_eq!(issues[0].title, "Fix the bug");
        assert_eq!(issues[0].state, "open");
    }

    #[tokio::test]
    async fn pull_issues_with_since_param() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/test/repo/issues"))
            .and(query_param("since", "2024-01-01T00:00:00Z"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&mock_server)
            .await;

        let sync = GithubSync::with_base_url("test-pat", "test/repo", mock_server.uri());
        let issues = sync
            .pull_issues(Some("2024-01-01T00:00:00Z"))
            .await
            .unwrap();

        assert!(issues.is_empty());
    }

    #[tokio::test]
    async fn push_comment_sends_post() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/test/repo/issues/99/comments"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 1})))
            .expect(1)
            .mount(&mock_server)
            .await;

        let sync = GithubSync::with_base_url("test-pat", "test/repo", mock_server.uri());
        sync.push_comment(99, "Hello from test").await.unwrap();
    }
}
