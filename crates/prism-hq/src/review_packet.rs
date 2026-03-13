/// ReviewPacket is the structured payload written by producers (CLI agent stop,
/// context-store checkout) into Completed inbox entry bodies, and enriched by
/// the HQ side (git diff, thread description, test output) before display in
/// the ApprovalGate modal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ReviewPacket {
    pub task_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub diff_preview: String,
    #[serde(default)]
    pub session_cost_usd: Option<f64>,
    #[serde(default)]
    pub test_summary: Option<String>,
    #[serde(default)]
    pub files_touched: Vec<String>,
    #[serde(default)]
    pub summary: String,
}

impl ReviewPacket {
    /// Parse an inbox entry body, falling back to plain-text for legacy entries.
    pub fn from_inbox_body(title: &str, body: &str) -> Self {
        serde_json::from_str(body).unwrap_or_else(|_| ReviewPacket {
            task_name: title.to_string(),
            description: body.to_string(),
            ..Default::default()
        })
    }

    /// Fill `description` from the named thread, if still empty.
    pub fn enrich_from_context(
        &mut self,
        handle: &crate::context_service::ContextHandle,
        thread_name: &str,
    ) {
        if self.description.is_empty() && !thread_name.is_empty() {
            if let Ok(thread) = handle.get_thread(thread_name) {
                self.description = thread.description;
            }
        }
    }

    /// Scan recent agent output lines for test result patterns.
    pub fn enrich_test_summary(&mut self, output_lines: &[String]) {
        if self.test_summary.is_none() {
            self.test_summary = extract_test_summary(output_lines);
        }
    }

    /// Run `git diff main...<branch>` synchronously and store a truncated preview.
    pub fn enrich_diff(&mut self, branch: &str) {
        if branch.is_empty() {
            return;
        }
        let result = std::process::Command::new("git")
            .args(["diff", &format!("main...{branch}")])
            .output();
        match result {
            Ok(out) if out.status.success() => {
                let diff = String::from_utf8(out.stdout)
                    .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
                self.diff_preview = if diff.is_empty() {
                    "(no diff — branch is up to date with main)".to_string()
                } else if diff.len() > 4000 {
                    let boundary = diff.floor_char_boundary(4000);
                    format!("{}…\n[truncated]", &diff[..boundary])
                } else {
                    diff
                };
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                self.diff_preview = format!("(git diff failed: {})", err.trim());
            }
            Err(e) => {
                self.diff_preview = format!("(git diff unavailable: {e})");
            }
        }
    }
}

/// Scan the last 100 lines for test-result keywords; return up to 5 matching lines.
pub fn extract_test_summary(lines: &[String]) -> Option<String> {
    const PATTERNS: &[&str] = &[
        "test result:",
        "FAILED",
        "PASSED",
        "tests passed",
        "error[E",
        "error: aborting",
    ];
    let tail_start = lines.len().saturating_sub(100);
    let mut result = String::new();
    let mut count = 0;
    for line in &lines[tail_start..] {
        if PATTERNS.iter().any(|p| line.contains(p)) {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(line);
            count += 1;
            if count >= 5 {
                break;
            }
        }
    }
    if result.is_empty() { None } else { Some(result) }
}
