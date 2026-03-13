//! Unix socket bridge for routing CLI approval prompts through Zed's UI.
//!
//! When `prism acp` (Zed) is running, it binds an `ApprovalListener` on a
//! per-project Unix domain socket. `prism run` (CLI) connects via `ApprovalClient`
//! and delegates permission prompts to Zed instead of the terminal.
//! If the socket doesn't exist, the CLI falls back to its TTY prompt.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub tool_name: String,
    pub args: serde_json::Value,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    AllowOnce,
    AllowSession,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub decision: ApprovalDecision,
}

// ---------------------------------------------------------------------------
// Socket path
// ---------------------------------------------------------------------------

/// Deterministic per-project socket path: `~/.prism/run/approval-<hash>.sock`
/// where `<hash>` is the first 12 hex chars of SHA-256(canonical cwd).
pub fn socket_path(cwd: &Path) -> PathBuf {
    let hash = crate::config::hash_path(cwd);
    crate::config::prism_home()
        .join("run")
        .join(format!("approval-{hash}.sock"))
}

// ---------------------------------------------------------------------------
// Length-delimited framing helpers
// ---------------------------------------------------------------------------

fn write_frame(writer: &mut impl Write, data: &[u8]) -> io::Result<()> {
    let len = (data.len() as u32).to_be_bytes();
    writer.write_all(&len)?;
    writer.write_all(data)?;
    writer.flush()
}

fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 10 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

async fn async_write_frame(
    writer: &mut (impl AsyncWriteExt + Unpin),
    data: &[u8],
) -> io::Result<()> {
    let len = (data.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(data).await?;
    writer.flush().await
}

async fn async_read_frame(reader: &mut (impl AsyncReadExt + Unpin)) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 10 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// ApprovalListener (async, used by ACP server)
// ---------------------------------------------------------------------------

pub struct ApprovalListener {
    listener: UnixListener,
    path: PathBuf,
}

impl ApprovalListener {
    /// Bind to the per-project approval socket.
    /// Creates parent dirs, detects stale sockets (connect-test then remove).
    pub fn bind(cwd: &Path) -> io::Result<Self> {
        let path = socket_path(cwd);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // If socket file exists, test if it's live
        if path.exists() {
            match StdUnixStream::connect(&path) {
                Ok(_) => {
                    // Another listener is alive — don't clobber it
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "another listener is active on this socket",
                    ));
                }
                Err(_) => {
                    // Stale socket — remove it
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

        let listener = UnixListener::bind(&path)?;
        tracing::info!(path = %path.display(), "approval bridge listener bound");
        Ok(Self { listener, path })
    }

    /// Accept loop: one connection at a time (Zed shows one dialog at a time).
    /// For each connection, reads a request, calls `handler`, writes the response.
    pub async fn serve<F, Fut>(self, handler: F)
    where
        F: Fn(ApprovalRequest) -> Fut,
        Fut: std::future::Future<Output = ApprovalResponse>,
    {
        loop {
            let (stream, _) = match self.listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!("approval bridge accept error: {e}");
                    continue;
                }
            };

            let (mut reader, mut writer) = stream.into_split();

            let frame = match async_read_frame(&mut reader).await {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("approval bridge read error: {e}");
                    continue;
                }
            };

            let req: ApprovalRequest = match serde_json::from_slice(&frame) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("approval bridge deserialize error: {e}");
                    continue;
                }
            };

            let resp = handler(req).await;

            let resp_bytes = match serde_json::to_vec(&resp) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("approval bridge serialize error: {e}");
                    continue;
                }
            };

            if let Err(e) = async_write_frame(&mut writer, &resp_bytes).await {
                tracing::warn!("approval bridge write error: {e}");
            }
        }
    }
}

impl Drop for ApprovalListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// ApprovalClient (sync, used by CLI)
// ---------------------------------------------------------------------------

pub struct ApprovalClient {
    stream: StdUnixStream,
}

impl ApprovalClient {
    /// Try to connect to the per-project approval socket.
    /// Returns `None` if the socket doesn't exist (Zed not running).
    pub fn try_connect(cwd: &Path) -> Option<Self> {
        let path = socket_path(cwd);
        let stream = StdUnixStream::connect(&path).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .ok()?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .ok()?;
        Some(Self { stream })
    }

    /// Send an approval request and wait for the response.
    /// Returns `None` on any I/O or protocol error.
    pub fn request_approval(&mut self, req: &ApprovalRequest) -> Option<ApprovalResponse> {
        let req_bytes = serde_json::to_vec(req).ok()?;
        write_frame(&mut self.stream, &req_bytes).ok()?;
        let resp_bytes = read_frame(&mut self.stream).ok()?;
        serde_json::from_slice(&resp_bytes).ok()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_deterministic() {
        let p1 = socket_path(Path::new("/tmp/test-project"));
        let p2 = socket_path(Path::new("/tmp/test-project"));
        assert_eq!(p1, p2);
    }

    #[test]
    fn socket_path_varies_by_cwd() {
        let p1 = socket_path(Path::new("/tmp/project-a"));
        let p2 = socket_path(Path::new("/tmp/project-b"));
        assert_ne!(p1, p2);
    }

    #[test]
    fn framing_roundtrip() {
        let data = b"hello world";
        let mut buf = Vec::new();
        write_frame(&mut buf, data).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let result = read_frame(&mut cursor).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn serde_roundtrip_request() {
        let req = ApprovalRequest {
            tool_name: "bash".to_string(),
            args: serde_json::json!({"command": "ls"}),
            title: "Run: ls".to_string(),
        };
        let bytes = serde_json::to_vec(&req).unwrap();
        let decoded: ApprovalRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.tool_name, "bash");
    }

    #[test]
    fn serde_roundtrip_response() {
        for decision in [
            ApprovalDecision::AllowOnce,
            ApprovalDecision::AllowSession,
            ApprovalDecision::Deny,
        ] {
            let resp = ApprovalResponse { decision };
            let bytes = serde_json::to_vec(&resp).unwrap();
            let decoded: ApprovalResponse = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(decoded.decision, decision);
        }
    }

    #[test]
    fn client_fallback_no_socket() {
        // No socket exists at this path — try_connect should return None
        let client = ApprovalClient::try_connect(Path::new("/tmp/nonexistent-prism-bridge-test"));
        assert!(client.is_none());
    }

    #[tokio::test]
    async fn listener_client_integration() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();

        let listener = ApprovalListener::bind(cwd).unwrap();

        let cwd_clone = cwd.to_path_buf();
        let handle = tokio::task::spawn(async move {
            listener
                .serve(|_req| async {
                    ApprovalResponse {
                        decision: ApprovalDecision::AllowOnce,
                    }
                })
                .await;
        });

        // Give the listener a moment to start accepting
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Client connects from a blocking context
        let resp = tokio::task::spawn_blocking(move || {
            let mut client = ApprovalClient::try_connect(&cwd_clone).expect("should connect");
            client
                .request_approval(&ApprovalRequest {
                    tool_name: "bash".into(),
                    args: serde_json::json!({"command": "rm -rf /"}),
                    title: "Run: rm -rf /".into(),
                })
                .expect("should get response")
        })
        .await
        .unwrap();

        assert_eq!(resp.decision, ApprovalDecision::AllowOnce);
        handle.abort();
    }

    #[tokio::test]
    async fn stale_socket_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        let path = socket_path(cwd);

        // Create parent dir + a stale file (no listener behind it)
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, b"stale").unwrap();

        // bind() should clean up the stale file and succeed
        let listener = ApprovalListener::bind(cwd).unwrap();
        assert!(path.exists());
        drop(listener);
        // Drop removes the socket
        assert!(!path.exists());
    }
}
