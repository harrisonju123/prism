use std::sync::OnceLock;
use tracing::warn;

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Fire a webhook notification. Non-blocking — spawns a tokio task.
/// Silently logs failures; does not propagate errors.
pub fn fire_webhook(url: &str, event_type: &str, payload: serde_json::Value) {
    if url.is_empty() {
        return;
    }
    let url = url.to_string();
    let event_type = event_type.to_string();
    let c = client().clone();

    tokio::spawn(async move {
        let body = serde_json::json!({
            "event": event_type,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "data": payload,
        });

        match c
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!(url = %url, status = %resp.status(), "webhook delivery failed");
                }
            }
            Err(e) => {
                warn!(url = %url, error = %e, "webhook request failed");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn fire_webhook_sends_correct_payload() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock_server)
            .await;

        fire_webhook(&mock_server.uri(), "task.created", json!({"id": "123"}));
        tokio::time::sleep(Duration::from_millis(200)).await;

        let requests = mock_server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["event"], "task.created");
        assert!(body.get("timestamp").is_some());
        assert_eq!(body["data"]["id"], "123");
    }

    #[tokio::test]
    async fn fire_webhook_empty_url_skips() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&mock_server)
            .await;

        fire_webhook("", "task.created", json!({}));

        mock_server.verify().await;
    }
}
