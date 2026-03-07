use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::types::*;

type HmacSha256 = Hmac<Sha256>;

pub fn sign_payload(payload: &serde_json::Value, secret: &str) -> String {
    let canonical = serde_json::to_string(payload).unwrap_or_default();
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(canonical.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn verify_signature(message: &ProtocolMessage, secret: &str) -> bool {
    let expected = sign_payload(&message.payload, secret);
    expected == message.signature
}

pub fn create_invocation(
    request: &InvocationRequest,
    sender: &str,
    secret: &str,
) -> ProtocolMessage {
    let payload = serde_json::to_value(request).unwrap();
    let signature = sign_payload(&payload, secret);
    ProtocolMessage {
        version: "1.0".into(),
        msg_type: MessageType::Invoke,
        sender: sender.to_string(),
        receiver: request.target_listing_id.clone(),
        payload,
        signature,
        timestamp: Utc::now(),
    }
}

pub fn create_response(
    response: &InvocationResponse,
    sender: &str,
    receiver: &str,
    secret: &str,
) -> ProtocolMessage {
    let payload = serde_json::to_value(response).unwrap();
    let signature = sign_payload(&payload, secret);
    ProtocolMessage {
        version: "1.0".into(),
        msg_type: MessageType::Response,
        sender: sender.to_string(),
        receiver: receiver.to_string(),
        payload,
        signature,
        timestamp: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn sign_verify_roundtrip() {
        let payload = serde_json::json!({"test": "data", "count": 42});
        let secret = "test-secret-key";
        let sig = sign_payload(&payload, secret);
        let msg = ProtocolMessage {
            version: "1.0".into(),
            msg_type: MessageType::Invoke,
            sender: "agent-a".into(),
            receiver: "agent-b".into(),
            payload,
            signature: sig,
            timestamp: Utc::now(),
        };
        assert!(verify_signature(&msg, secret));
    }

    #[test]
    fn tampered_payload_fails() {
        let payload = serde_json::json!({"test": "data"});
        let secret = "test-secret-key";
        let sig = sign_payload(&payload, secret);
        let msg = ProtocolMessage {
            version: "1.0".into(),
            msg_type: MessageType::Invoke,
            sender: "agent-a".into(),
            receiver: "agent-b".into(),
            payload: serde_json::json!({"test": "tampered"}),
            signature: sig,
            timestamp: Utc::now(),
        };
        assert!(!verify_signature(&msg, secret));
    }

    #[test]
    fn wrong_secret_fails() {
        let payload = serde_json::json!({"test": "data"});
        let sig = sign_payload(&payload, "secret-1");
        let msg = ProtocolMessage {
            version: "1.0".into(),
            msg_type: MessageType::Invoke,
            sender: "agent-a".into(),
            receiver: "agent-b".into(),
            payload,
            signature: sig,
            timestamp: Utc::now(),
        };
        assert!(!verify_signature(&msg, "secret-2"));
    }

    #[test]
    fn create_invocation_message() {
        let req = InvocationRequest {
            caller_agent_id: "caller".into(),
            target_listing_id: "target".into(),
            method: "process".into(),
            params: serde_json::json!({"input": "hello"}),
            max_cost: Some(0.01),
            timeout_s: Some(30),
            trace_id: None,
        };
        let msg = create_invocation(&req, "caller", "secret");
        assert_eq!(msg.msg_type, MessageType::Invoke);
        assert_eq!(msg.sender, "caller");
        assert_eq!(msg.receiver, "target");
        assert!(verify_signature(&msg, "secret"));
    }

    #[test]
    fn create_response_message() {
        let resp = InvocationResponse {
            request_id: Uuid::new_v4(),
            status: InvocationStatus::Success,
            result: serde_json::json!({"output": "world"}),
            cost: 0.005,
            latency_ms: 150,
            target_framework: Some("langchain".into()),
        };
        let msg = create_response(&resp, "target", "caller", "secret");
        assert_eq!(msg.msg_type, MessageType::Response);
        assert!(verify_signature(&msg, "secret"));
    }
}
