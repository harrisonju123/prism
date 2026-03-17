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
    /// Change sets recorded during this mission (grouped by file).
    #[serde(default)]
    pub change_sets: Vec<prism_context::model::ChangeSet>,
    /// Plan assumptions at time of review.
    #[serde(default)]
    pub assumptions: Vec<prism_context::model::Assumption>,
    /// Validation summary derived from work package evidence.
    #[serde(default)]
    pub validation_summary: Option<String>,
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
    /// Also enriches change_sets and assumptions from the active plan.
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

        // Enrich with active plan data (change sets, assumptions, validation summary)
        if let Ok(Some(plan)) = handle.get_active_plan() {
            if self.assumptions.is_empty() {
                self.assumptions = plan.assumptions.clone();
            }

            if self.change_sets.is_empty() {
                if let Ok(sets) = handle.list_change_sets(Some(plan.id), None) {
                    self.change_sets = sets;
                }
            }

            if self.validation_summary.is_none() {
                use prism_context::model::{ValidationStatus, WorkPackageStatus};
                if let Ok(wps) = handle.list_work_packages(Some(plan.id), None) {
                    if !wps.is_empty() {
                        let (mut done, mut passing, mut failing) = (0, 0, 0);
                        for w in &wps {
                            if w.status == WorkPackageStatus::Done { done += 1; }
                            if w.validation_status == ValidationStatus::Passing { passing += 1; }
                            if w.validation_status == ValidationStatus::Failing { failing += 1; }
                        }
                        self.validation_summary = Some(format!(
                            "{}/{} WPs done · {} passing · {} failing",
                            done, wps.len(), passing, failing
                        ));
                    }
                }
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
