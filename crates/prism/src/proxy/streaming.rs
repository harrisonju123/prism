use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::types::{ChatCompletionChunk, PrismStreamError, Usage};

/// Wraps a provider's byte stream, relays SSE chunks to the client,
/// and extracts the final usage from the stream.
pub struct StreamRelay {
    /// Receiver for processed SSE bytes to send to client.
    rx: mpsc::Receiver<Result<Bytes, PrismStreamError>>,
}

/// Accumulated data extracted from the stream after it completes.
#[derive(Debug, Default)]
pub struct StreamResult {
    pub usage: Option<Usage>,
    pub completion_text: String,
    pub model: String,
    /// Time to first token in milliseconds (from stream start to first content chunk).
    pub ttft_ms: Option<u32>,
}

impl StreamRelay {
    /// Start relaying a provider stream.
    /// Returns (StreamRelay for axum body, oneshot receiver for final result).
    pub fn start(
        mut source: Pin<Box<dyn Stream<Item = Result<Bytes, PrismStreamError>> + Send>>,
    ) -> (Self, tokio::sync::oneshot::Receiver<StreamResult>) {
        let (tx, rx) = mpsc::channel::<Result<Bytes, PrismStreamError>>(64);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            use futures::StreamExt;

            let mut result = StreamResult::default();
            let mut buffer = String::new();
            let stream_start = Instant::now();
            let mut first_content_seen = false;

            while let Some(chunk_result) = source.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        // Forward raw bytes to client
                        if tx.send(Ok(bytes.clone())).await.is_err() {
                            break; // client disconnected
                        }

                        // Parse SSE data lines to extract usage
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(line_end) = buffer.find('\n') {
                            let line = buffer[..line_end].trim().to_string();
                            buffer.drain(..line_end + 1);

                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    continue;
                                }
                                if let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data)
                                {
                                    if result.model.is_empty() {
                                        result.model = chunk.model.clone();
                                    }
                                    if let Some(usage) = chunk.usage {
                                        result.usage = Some(usage);
                                    }
                                    // Accumulate completion text for hashing
                                    for choice in &chunk.choices {
                                        if let Some(content) =
                                            choice.delta.get("content").and_then(|c| c.as_str())
                                        {
                                            // Measure TTFT on first content chunk
                                            if !first_content_seen && !content.is_empty() {
                                                first_content_seen = true;
                                                result.ttft_ms =
                                                    Some(stream_start.elapsed().as_millis() as u32);
                                            }
                                            result.completion_text.push_str(content);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                }
            }

            let _ = result_tx.send(result);
        });

        (Self { rx }, result_rx)
    }
}

impl Stream for StreamRelay {
    type Item = Result<Bytes, PrismStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use serde_json::json;

    /// Helper: build an SSE data line from a ChatCompletionChunk JSON value.
    fn sse_line(data: &serde_json::Value) -> Bytes {
        Bytes::from(format!("data: {}\n\n", data))
    }

    fn make_chunk(
        model: &str,
        content: Option<&str>,
        usage: Option<serde_json::Value>,
    ) -> serde_json::Value {
        let mut delta = json!({});
        if let Some(c) = content {
            delta["content"] = json!(c);
        }
        let mut chunk = json!({
            "id": "chatcmpl-test",
            "object": "chat.completion.chunk",
            "created": 1700000000i64,
            "model": model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": null}]
        });
        if let Some(u) = usage {
            chunk["usage"] = u;
        }
        chunk
    }

    fn mock_stream(
        chunks: Vec<Bytes>,
    ) -> Pin<Box<dyn Stream<Item = Result<Bytes, PrismStreamError>> + Send>> {
        Box::pin(stream::iter(
            chunks
                .into_iter()
                .map(|b| Ok(b) as Result<Bytes, PrismStreamError>),
        ))
    }

    #[tokio::test]
    async fn test_usage_extraction() {
        let usage_json = json!({
            "prompt_tokens": 10,
            "completion_tokens": 20,
            "total_tokens": 30
        });
        let chunks = vec![
            sse_line(&make_chunk("gpt-4o", Some("hi"), None)),
            sse_line(&make_chunk("gpt-4o", None, Some(usage_json))),
        ];

        let (_relay, result_rx) = StreamRelay::start(mock_stream(chunks));
        let result = result_rx.await.unwrap();

        let usage = result.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 20);
        assert_eq!(usage.total_tokens, 30);
    }

    #[tokio::test]
    async fn test_text_accumulation() {
        let chunks = vec![
            sse_line(&make_chunk("gpt-4o", Some("Hello"), None)),
            sse_line(&make_chunk("gpt-4o", Some(", "), None)),
            sse_line(&make_chunk("gpt-4o", Some("world!"), None)),
        ];

        let (_relay, result_rx) = StreamRelay::start(mock_stream(chunks));
        let result = result_rx.await.unwrap();

        assert_eq!(result.completion_text, "Hello, world!");
    }

    #[tokio::test]
    async fn test_model_capture_from_first_chunk() {
        let chunks = vec![
            sse_line(&make_chunk("gpt-4o-2024-05-13", Some("a"), None)),
            sse_line(&make_chunk("gpt-4o-2024-05-13-v2", Some("b"), None)),
        ];

        let (_relay, result_rx) = StreamRelay::start(mock_stream(chunks));
        let result = result_rx.await.unwrap();

        // Model should be captured from the first chunk only
        assert_eq!(result.model, "gpt-4o-2024-05-13");
    }

    #[tokio::test]
    async fn test_done_handling() {
        let chunks = vec![
            sse_line(&make_chunk("gpt-4o", Some("done"), None)),
            Bytes::from("data: [DONE]\n\n"),
        ];

        let (_relay, result_rx) = StreamRelay::start(mock_stream(chunks));
        let result = result_rx.await.unwrap();

        // Should complete without panicking and have accumulated text
        assert_eq!(result.completion_text, "done");
    }

    #[tokio::test]
    async fn test_ttft_measurement() {
        // First chunk has empty content (role-only delta), second has actual content.
        let role_chunk = json!({
            "id": "chatcmpl-test",
            "object": "chat.completion.chunk",
            "created": 1700000000i64,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]
        });
        let chunks = vec![
            sse_line(&role_chunk),
            sse_line(&make_chunk("gpt-4o", Some("Hello"), None)),
        ];

        let (_relay, result_rx) = StreamRelay::start(mock_stream(chunks));
        let result = result_rx.await.unwrap();

        // TTFT should be set (non-None) once the first content chunk arrives
        assert!(result.ttft_ms.is_some());
        // The value should be small since we're in a test (likely < 100ms)
        assert!(result.ttft_ms.unwrap() < 1000);
    }
}
