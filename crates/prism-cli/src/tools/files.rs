use std::path::Path;

pub async fn read_file(path: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => return format!("error reading {path}: {e}"),
    };

    match (offset, limit) {
        (None, None) => contents,
        _ => {
            let start = offset.unwrap_or(1).saturating_sub(1); // 0-indexed
            let lines: Vec<&str> = contents.lines().collect();
            let slice = if let Some(n) = limit {
                &lines[start.min(lines.len())..lines.len().min(start + n)]
            } else {
                &lines[start.min(lines.len())..]
            };
            // Prefix with line numbers like cat -n
            slice
                .iter()
                .enumerate()
                .map(|(i, l)| format!("{:>6}\t{l}", start + i + 1))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

pub async fn write_file(path: &str, content: &str) -> String {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return format!("error creating directories for {path}: {e}");
            }
        }
    }
    match tokio::fs::write(path, content).await {
        Ok(()) => format!("wrote {} bytes to {path}", content.len()),
        Err(e) => format!("error writing {path}: {e}"),
    }
}

pub async fn edit_file(path: &str, old_string: &str, new_string: &str) -> String {
    if old_string.is_empty() {
        return "error: old_string must not be empty".to_string();
    }
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => return format!("error reading {path}: {e}"),
    };
    let count = contents.matches(old_string).count();
    if count == 0 {
        return format!(
            "error: old_string not found in {path}\n\nCurrent file contents:\n{contents}"
        );
    }
    if count > 1 {
        return format!(
            "error: old_string appears {count} times in {path}; provide more context to make it unique\n\nCurrent file contents:\n{contents}"
        );
    }
    let new_contents = contents.replacen(old_string, new_string, 1);
    match tokio::fs::write(path, &new_contents).await {
        Ok(()) => format!("edited {path}"),
        Err(e) => format!("error writing {path}: {e}"),
    }
}

pub async fn list_dir(path: &str) -> String {
    let read = match std::fs::read_dir(path) {
        Ok(r) => r,
        Err(e) => return format!("error listing {path}: {e}"),
    };

    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();

    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => dirs.push(format!("{name}/")),
            _ => files.push(name),
        }
    }

    dirs.sort();
    files.sort();
    let entries: Vec<String> = dirs.into_iter().chain(files).collect();

    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn edit_file_success() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello world").unwrap();
        let path = f.path().to_str().unwrap();
        let result = edit_file(path, "world", "rust").await;
        assert_eq!(result, format!("edited {path}"));
        let contents = tokio::fs::read_to_string(path).await.unwrap();
        assert_eq!(contents, "hello rust");
    }

    #[tokio::test]
    async fn edit_file_not_found() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello world").unwrap();
        let path = f.path().to_str().unwrap();
        let result = edit_file(path, "missing", "x").await;
        assert!(result.starts_with("error: old_string not found"));
    }

    #[tokio::test]
    async fn edit_file_ambiguous() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "foo foo foo").unwrap();
        let path = f.path().to_str().unwrap();
        let result = edit_file(path, "foo", "bar").await;
        assert!(result.contains("appears 3 times"));
    }
}
