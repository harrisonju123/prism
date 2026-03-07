pub async fn web_fetch(url: &str) -> String {
    let client = match reqwest::Client::builder()
        .user_agent("prism-cli/0.1")
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => return format!("error building http client: {e}"),
    };

    let resp = match client.get(url).send().await {
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
