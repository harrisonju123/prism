use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::Client;
use reqwest::Url;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{CapturedLog, ReplayRun, RequestReplay, RequestReplayBundle};

const DEFAULT_REPLAY_DIR: &str = "request-replay";

pub async fn run(request_id: &str, env: &str, log_dir: Option<&str>) -> Result<()> {
    let replay_dir =
        std::env::var("PRISM_REPLAY_DIR").unwrap_or_else(|_| DEFAULT_REPLAY_DIR.into());
    let replay_dir = PathBuf::from(replay_dir);

    let bundle = load_bundle(&replay_dir)?;
    let request = load_request(&replay_dir, request_id)?;

    let base_url = bundle
        .base_urls
        .get(env)
        .cloned()
        .with_context(|| format!("unknown replay env '{env}'"))?;

    let variant_id =
        std::env::var("PRISM_REPLAY_VARIANT").unwrap_or_else(|_| "happy-path".to_string());
    let variant = request
        .variants
        .iter()
        .find(|v| v.id == variant_id)
        .or_else(|| request.variants.first())
        .with_context(|| format!("no variants defined for request '{request_id}'"))?;

    let url = build_url(
        &base_url,
        &request.path,
        &variant.request.path_params,
        &variant.request.query,
    )?;
    let client = Client::new();

    let mut req = client.request(request.method.parse()?, url.clone());

    if let Some(auth) = request.auth.as_ref()
        && auth.required
        && let Some((header, value)) = resolve_auth_header(&bundle, &auth.scheme_id)?
    {
        req = req.header(header, value);
    }

    for (name, value) in &variant.request.headers {
        if let Some(s) = value.as_str() {
            req = req.header(name, s);
        } else {
            req = req.header(name, value.to_string());
        }
    }

    if let Some(body) = variant.request.body.as_ref() {
        req = req.json(body);
    }

    let start = Instant::now();
    let response = req.send().await;
    let latency_ms = start.elapsed().as_millis();

    let (status, response_body, error) = match response {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            let json = serde_json::from_str::<Value>(&text).ok();
            (Some(status), json, None)
        }
        Err(err) => (None, None, Some(err.to_string())),
    };

    let run = ReplayRun {
        request_id: request_id.to_string(),
        variant_id: variant.id.clone(),
        env: env.to_string(),
        url: url.to_string(),
        method: request.method.clone(),
        status,
        ok: status
            .map(|s| s == request.expected.status)
            .unwrap_or(false),
        latency_ms: Some(latency_ms),
        response_body,
        error,
        captured_logs: Vec::<CapturedLog>::new(),
    };

    let log_path = write_run_log(&replay_dir, log_dir, &run)?;
    println!("{}", serde_json::to_string_pretty(&run)?);
    eprintln!("saved replay log to {}", log_path.display());

    Ok(())
}

fn load_bundle(replay_dir: &Path) -> Result<RequestReplayBundle> {
    let index_path = replay_dir.join("index.json");
    let payload = fs::read_to_string(&index_path)
        .with_context(|| format!("failed to read {}", index_path.display()))?;
    let bundle: RequestReplayBundle = serde_json::from_str(&payload)
        .with_context(|| format!("invalid JSON in {}", index_path.display()))?;
    Ok(bundle)
}

fn load_request(replay_dir: &Path, request_id: &str) -> Result<RequestReplay> {
    let request_path = replay_dir
        .join("requests")
        .join(format!("{request_id}.json"));
    let payload = fs::read_to_string(&request_path)
        .with_context(|| format!("failed to read {}", request_path.display()))?;
    let request: RequestReplay = serde_json::from_str(&payload)
        .with_context(|| format!("invalid JSON in {}", request_path.display()))?;
    Ok(request)
}

fn resolve_auth_header(
    bundle: &RequestReplayBundle,
    scheme_id: &str,
) -> Result<Option<(String, String)>> {
    let scheme = bundle
        .auth
        .schemes
        .iter()
        .find(|s| s.id == scheme_id)
        .or_else(|| {
            bundle
                .auth
                .default
                .as_ref()
                .and_then(|d| bundle.auth.schemes.iter().find(|s| &s.id == d))
        });

    let Some(scheme) = scheme else {
        return Ok(None);
    };

    let header = scheme
        .header
        .clone()
        .unwrap_or_else(|| "Authorization".to_string());
    let env_var = scheme
        .env_var
        .clone()
        .unwrap_or_else(|| "PRISM_API_KEY".to_string());
    let token = std::env::var(&env_var).unwrap_or_default();

    if token.is_empty() {
        anyhow::bail!("auth required but env var '{}' is empty", env_var);
    }

    let value = if let Some(prefix) = scheme.prefix.as_ref() {
        format!("{prefix} {token}")
    } else {
        token
    };

    Ok(Some((header, value)))
}

fn build_url(
    base: &str,
    path: &str,
    path_params: &BTreeMap<String, Value>,
    query: &BTreeMap<String, Value>,
) -> Result<Url> {
    let mut url = Url::parse(base).with_context(|| format!("invalid base url '{base}'"))?;

    let mut resolved_path = path.to_string();
    for (key, value) in path_params {
        let placeholder = format!("{{{}}}", key);
        let value_str = value
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| value.to_string());
        resolved_path = resolved_path.replace(&placeholder, &value_str);
    }

    let full_path = if resolved_path.starts_with('/') {
        resolved_path
    } else {
        format!("/{resolved_path}")
    };
    url.set_path(&full_path);

    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            if let Some(s) = value.as_str() {
                pairs.append_pair(key, s);
            } else {
                pairs.append_pair(key, &value.to_string());
            }
        }
    }

    Ok(url)
}

fn write_run_log(replay_dir: &Path, log_dir: Option<&str>, run: &ReplayRun) -> Result<PathBuf> {
    let dir = if let Some(log_dir) = log_dir {
        PathBuf::from(log_dir)
    } else {
        replay_dir.join("runs")
    };
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let timestamp = Utc::now().format("%Y%m%dT%H%M%S");
    let filename = format!("{}-{}-{}.json", run.request_id, run.variant_id, timestamp);
    let path = dir.join(filename);
    fs::write(&path, serde_json::to_string_pretty(run)?)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_build_url_path_params() {
        let path_params = BTreeMap::from([
            ("user_id".to_string(), json!("abc")),
        ]);
        let query = BTreeMap::new();
        let url = build_url("http://localhost:9100", "/users/{user_id}", &path_params, &query).unwrap();
        assert_eq!(url.path(), "/users/abc");
        assert_eq!(url.query(), None);
    }

    #[test]
    fn test_build_url_query_params() {
        let path_params = BTreeMap::new();
        let query = BTreeMap::from([
            ("limit".to_string(), json!(10)),
            ("offset".to_string(), json!(0)),
        ]);
        let url = build_url("http://localhost:9100", "/users", &path_params, &query).unwrap();
        assert_eq!(url.path(), "/users");
        assert!(url.query().unwrap().contains("limit=10"));
        assert!(url.query().unwrap().contains("offset=0"));
    }

    #[test]
    fn test_build_url_leading_slash() {
        let url = build_url("http://localhost:9100", "users", &BTreeMap::new(), &BTreeMap::new()).unwrap();
        assert_eq!(url.path(), "/users");
    }

    #[test]
    fn test_build_url_combined() {
        let path_params = BTreeMap::from([("id".to_string(), json!("42"))]);
        let query = BTreeMap::from([("fields".to_string(), json!("name,email"))]);
        let url = build_url("http://localhost:9100", "/users/{id}", &path_params, &query).unwrap();
        assert_eq!(url.path(), "/users/42");
        assert!(url.query().unwrap().contains("fields=name%2Cemail"));
    }
}
