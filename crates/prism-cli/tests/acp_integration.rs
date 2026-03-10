use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::Duration;

fn send_jsonrpc(stdin: &mut impl Write, msg: &serde_json::Value) {
    let body = serde_json::to_string(msg).unwrap();
    writeln!(stdin, "{body}").unwrap();
    stdin.flush().unwrap();
}

fn read_jsonrpc(reader: &mut BufReader<impl std::io::Read>) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

fn initialize_request(id: u64) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": "0.1",
            "clientInfo": { "name": "test", "version": "0.0.1" },
            "capabilities": {}
        }
    })
}

/// Spawn the ACP server and set up a reader thread that forwards responses via mpsc.
/// Returns (child, stdin, response_receiver).
fn setup_acp(expected_responses: usize) -> (std::process::Child, std::process::ChildStdin, Receiver<serde_json::Value>) {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| {
        dirs::home_dir()
            .unwrap()
            .join(".cargo/bin/cargo")
            .to_string_lossy()
            .into_owned()
    });
    let mut child = Command::new(cargo)
        .args(["run", "-p", "prism-cli", "--", "acp"])
        .env("PRISM_URL", "http://localhost:0")
        .env("PRISM_API_KEY", "test-key")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn prism acp");

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        for _ in 0..expected_responses {
            let resp = read_jsonrpc(&mut reader);
            if tx.send(resp).is_err() {
                break;
            }
        }
    });

    (child, stdin, rx)
}

const TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn test_initialize() {
    let (mut child, mut stdin, rx) = setup_acp(1);

    send_jsonrpc(&mut stdin, &initialize_request(1));

    let resp = rx.recv_timeout(TIMEOUT).expect("timed out on initialize");

    assert_eq!(resp["id"], 1);
    let result = &resp["result"];
    assert!(result["protocolVersion"].is_number() || result["protocolVersion"].is_string());
    assert_eq!(result["agentInfo"]["name"], "prism");
    assert_eq!(result["agentCapabilities"]["loadSession"], true);

    drop(stdin);
    let _ = child.kill();
}

#[test]
fn test_new_session() {
    let (mut child, mut stdin, rx) = setup_acp(2);

    send_jsonrpc(&mut stdin, &initialize_request(1));
    rx.recv_timeout(TIMEOUT).expect("timed out on initialize");

    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string();
    send_jsonrpc(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": { "cwd": cwd, "mcpServers": [] }
        }),
    );

    let resp = rx.recv_timeout(TIMEOUT).expect("timed out on session/new");

    assert_eq!(resp["id"], 2);
    let session_id = resp["result"]["sessionId"].as_str().unwrap();
    assert!(
        uuid::Uuid::parse_str(session_id).is_ok(),
        "session ID should be a valid UUID: {session_id}"
    );

    drop(stdin);
    let _ = child.kill();
}

#[test]
fn test_prompt_without_session() {
    let (mut child, mut stdin, rx) = setup_acp(2);

    send_jsonrpc(&mut stdin, &initialize_request(1));
    rx.recv_timeout(TIMEOUT).expect("timed out on initialize");

    send_jsonrpc(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/prompt",
            "params": {
                "sessionId": "nonexistent-session-id",
                "prompt": [{ "type": "text", "text": "hello" }]
            }
        }),
    );

    let resp = rx.recv_timeout(TIMEOUT).expect("timed out on session/prompt");

    assert_eq!(resp["id"], 2);
    assert!(
        resp["error"].is_object(),
        "expected error response for bogus session, got: {resp}"
    );

    drop(stdin);
    let _ = child.kill();
}
