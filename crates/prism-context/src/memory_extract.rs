use crate::store::Store;
use std::collections::HashSet;
use uuid::Uuid;

/// Keywords indicating error resolution patterns in session content.
pub const RESOLUTION_KEYWORDS: &[&str] =
    &["fixed", "resolved", "workaround", "root cause", "solution"];

/// Auto-extract memories from session data on checkout.
/// Pure heuristics — no LLM involved.
pub async fn auto_extract_memories(
    store: &dyn Store,
    workspace_id: Uuid,
    agent_name: &str,
    next_steps: &[String],
    files_touched: &[String],
    findings: &[String],
    thread_name: Option<&str>,
) -> Vec<String> {
    let mut saved = Vec::new();
    let thread_tag = thread_name.unwrap_or("global");

    // 1. Next-steps → memories
    for step in next_steps {
        let hash = &format!("{:x}", dedup_hash(step))[..8];
        let key = format!("next_step:{thread_tag}:{hash}");
        if store
            .save_memory(
                workspace_id,
                &key,
                step,
                None,
                agent_name,
                vec!["next_step".to_string(), thread_tag.to_string()],
            )
            .await
            .is_ok()
        {
            saved.push(key);
        }
    }

    // 2. File co-modification patterns
    if files_touched.len() >= 3 {
        let dirs: HashSet<String> = files_touched
            .iter()
            .filter_map(|f| {
                let parts: Vec<&str> = f.rsplitn(2, '/').collect();
                if parts.len() == 2 {
                    Some(parts[1].to_string())
                } else {
                    None
                }
            })
            .collect();
        if dirs.len() >= 3 {
            let mut sorted_dirs: Vec<&str> = dirs.iter().map(|s| s.as_str()).collect();
            sorted_dirs.sort();
            let hash = &format!("{:x}", dedup_hash(&sorted_dirs.join(",")))[..8];
            let key = format!("file_pattern:{hash}");
            let value = format!(
                "Files spanning {} directories modified together: {}",
                dirs.len(),
                files_touched.join(", ")
            );
            if store
                .save_memory(
                    workspace_id,
                    &key,
                    &value,
                    None,
                    agent_name,
                    vec!["file_pattern".to_string()],
                )
                .await
                .is_ok()
            {
                saved.push(key);
            }
        }
    }

    // 3. Error resolution patterns
    for finding in findings {
        let lower = finding.to_lowercase();
        if RESOLUTION_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
            let hash = &format!("{:x}", dedup_hash(finding))[..8];
            let key = format!("resolution:{hash}");
            if store
                .save_memory(
                    workspace_id,
                    &key,
                    finding,
                    None,
                    agent_name,
                    vec!["resolution".to_string(), thread_tag.to_string()],
                )
                .await
                .is_ok()
            {
                saved.push(key);
            }
        }
    }

    saved
}

/// FNV-1a hash for stable dedup keys across Rust versions.
pub fn dedup_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for byte in s.as_bytes() {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
