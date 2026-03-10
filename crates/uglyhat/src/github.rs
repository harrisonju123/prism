use reqwest::header;
use serde::Deserialize;

pub struct GithubSync {
    client: reqwest::Client,
    repo: String,
}

#[derive(Debug, Deserialize)]
pub struct GithubIssue {
    pub number: i64,
    pub title: String,
    pub body: Option<String>,
}

impl GithubSync {
    pub fn new(pat: &str, repo: &str) -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static("uglyhat"),
        );
        if let Ok(val) = header::HeaderValue::from_str(&format!("Bearer {pat}")) {
            headers.insert(header::AUTHORIZATION, val);
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("failed to build HTTP client");

        Self {
            client,
            repo: repo.to_string(),
        }
    }

    pub async fn pull_issues(
        &self,
        since: Option<&str>,
    ) -> Result<Vec<GithubIssue>, Box<dyn std::error::Error + Send + Sync>> {
        let mut url = format!(
            "https://api.github.com/repos/{}/issues?state=open&per_page=100",
            self.repo
        );
        if let Some(since) = since {
            url.push_str(&format!("&since={since}"));
        }

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("GitHub API {status}: {body}").into());
        }

        let issues: Vec<GithubIssue> = resp.json().await?;
        Ok(issues)
    }
}
