use std::path::Path;

pub async fn read_file(path: &str) -> String {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(e) => format!("error reading {path}: {e}"),
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
