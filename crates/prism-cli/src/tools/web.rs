use std::fmt::Write as _;
use std::sync::OnceLock;

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("prism-cli/0.1")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client")
    })
}

pub async fn web_search(query: &str, count: Option<u32>) -> String {
    if query.trim().is_empty() {
        return "error: query must not be empty".to_string();
    }

    let api_key = match std::env::var("BRAVE_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            return "error: BRAVE_API_KEY environment variable not set. \
                Sign up for a free API key at https://brave.com/search/api/ \
                (free tier: 1,000 queries/month, no credit card required)."
                .to_string()
        }
    };

    let count = count.unwrap_or(5).clamp(1, 20);
    let count_str = count.to_string();

    let resp = match http_client()
        .get("https://api.search.brave.com/res/v1/web/search")
        .query(&[("q", query), ("count", &count_str)])
        .header("X-Subscription-Token", &api_key)
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("error contacting Brave Search API: {e}"),
    };

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return "error: Brave Search API key is invalid or expired (401/403). \
            Check your BRAVE_API_KEY."
            .to_string();
    }
    if !status.is_success() {
        return format!("error: Brave Search API returned HTTP {status}");
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return format!("error parsing Brave Search response: {e}"),
    };

    let results = match body["web"]["results"].as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return format!("no results found for query: {query}"),
    };

    let mut output = format!("Search results for \"{query}\":\n\n");
    for (i, result) in results.iter().enumerate() {
        let title = result["title"].as_str().unwrap_or("(no title)");
        let url = result["url"].as_str().unwrap_or("");
        let desc = result["description"].as_str().unwrap_or("(no description)");
        let _ = write!(output, "{}. {}\n   {}\n   {}\n\n", i + 1, title, url, desc);
    }

    output
}

pub async fn web_fetch(url: &str) -> String {
    let resp = match http_client().get(url).send().await {
        Ok(r) => r,
        Err(e) => return format!("error fetching {url}: {e}"),
    };

    let status = resp.status().as_u16();
    let body = match resp.text().await {
        Ok(t) => t,
        Err(e) => return format!("error reading response body: {e}"),
    };

    // Strip HTML tags so the model gets readable text.
    let text = strip_html(&body);

    format!("HTTP {status}\n\n{text}")
}

fn strip_html(html: &str) -> String {
    // Remove <script> blocks
    let re_script = regex::Regex::new(r"(?si)<script[^>]*>.*?</script>").unwrap();
    let s = re_script.replace_all(html, " ");
    // Remove <style> blocks
    let re_style = regex::Regex::new(r"(?si)<style[^>]*>.*?</style>").unwrap();
    let s = re_style.replace_all(&s, " ");
    // Strip all remaining tags
    let re_tags = regex::Regex::new(r"<[^>]+>").unwrap();
    let s = re_tags.replace_all(&s, " ");
    // Decode common HTML entities
    let s = s
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    // Collapse whitespace
    let re_ws = regex::Regex::new(r"[ \t]{2,}").unwrap();
    let s = re_ws.replace_all(&s, " ");
    let re_nl = regex::Regex::new(r"\n{3,}").unwrap();
    re_nl.replace_all(s.trim(), "\n\n").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_web_search_empty_query() {
        let result = web_search("", None).await;
        assert!(result.contains("empty"), "expected empty-query error, got: {result}");
    }

    #[tokio::test]
    async fn test_web_search_missing_api_key() {
        // SAFETY: affects process-global env; safe only when BRAVE_API_KEY is not set by other
        // concurrent tests. In CI this var is unset, making remove_var a no-op. If the var IS
        // set in the environment, run this test with `cargo test -- --test-threads=1`.
        unsafe { std::env::remove_var("BRAVE_API_KEY") };
        let result = web_search("rust async", None).await;
        assert!(
            result.contains("BRAVE_API_KEY"),
            "expected missing-key error, got: {result}"
        );
    }

    #[test]
    fn test_strip_html_basic() {
        let html = "<p>Hello <b>world</b></p>";
        let result = strip_html(html);
        assert!(result.contains("Hello"));
        assert!(result.contains("world"));
        assert!(!result.contains("<"));
        assert!(!result.contains(">"));
    }

    #[test]
    fn test_strip_html_entities() {
        let html = "&amp; &lt; &gt; &nbsp;";
        let result = strip_html(html);
        assert!(result.contains("& < >"));
    }

    #[test]
    fn test_strip_html_script() {
        let html = "before <script>alert('xss')</script> after";
        let result = strip_html(html);
        assert!(!result.contains("script"));
        assert!(result.contains("before"));
        assert!(result.contains("after"));
    }
}
