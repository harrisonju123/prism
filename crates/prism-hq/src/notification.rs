/// Fire a macOS system notification. Fire-and-forget; errors are silently ignored.
#[cfg(target_os = "macos")]
pub fn notify_os(title: &str, body: &str) {
    let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");
    let escaped_title = title.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display notification \"{escaped_body}\" with title \"{escaped_title}\""
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .spawn();
}

#[cfg(not(target_os = "macos"))]
pub fn notify_os(_title: &str, _body: &str) {}
