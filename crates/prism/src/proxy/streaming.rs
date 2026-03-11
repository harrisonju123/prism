use axum::response::sse::Event;
use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::types::{ChatCompletionChunk, PrismStreamError, Usage};

/// Build an SSE error event from a stream error. Used by both OpenAI and
/// Anthropic handlers to surface upstream failures to the client.
pub fn stream_error_event(e: &PrismStreamError) -> Event {
    tracing::warn!(error = %e, "upstream provider stream error, sending error event to client");
    let error_payload = serde_json::json!({
        "error": {
            "message": format!("upstream stream error: {e}"),
            "type": "stream_error",
        }
    });
    Event::default()
        .event("error")
        .data(error_payload.to_string())
}

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
        idle_timeout: Duration,
    ) -> (Self, tokio::sync::oneshot::Receiver<StreamResult>) {
        let (tx, rx) = mpsc::channel::<Result<Bytes, PrismStreamError>>(64);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            use futures::StreamExt;
            use tokio::time::sleep;

            let mut result = StreamResult::default();
            let mut buffer = String::new();
            let stream_start = Instant::now();
            let mut first_content_seen = false;

            // Single sleep future, reset on each chunk — avoids allocating a
            // new timer per iteration.
            let idle_sleep = sleep(idle_timeout);
            tokio::pin!(idle_sleep);

            loop {
                let chunk_result = tokio::select! {
                    maybe_chunk = source.next() => {
                        match maybe_chunk {
                            Some(chunk) => chunk,
                            None => break, // stream ended naturally
                        }
                    }
                    _ = &mut idle_sleep => {
                        tracing::warn!(
                            idle_timeout_secs = idle_timeout.as_secs(),
                            "provider stream idle timeout, terminating"
                        );
                        let _ = tx
                            .send(Err(PrismStreamError::Other(
                                "provider stream idle timeout".into(),
                            )))
                            .await;
                        break;
                    }
                };
                // Reset idle deadline after each successful receive
                idle_sleep.as_mut().reset(tokio::time::Instant::now() + idle_timeout);
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
                                        // Regular tokens use last-write-wins (OpenAI sends
                                        // absolute values in the final chunk).  Cache tokens
                                        // are accumulated across chunks because Anthropic may
                                        // report them incrementally.
                                        let prev_cache_read = result
                                            .usage
                                            .as_ref()
                                            .map(|u| u.cache_read_input_tokens)
                                            .unwrap_or(0);
                                        let prev_cache_creation = result
                                            .usage
                                            .as_ref()
                                            .map(|u| u.cache_creation_input_tokens)
                                            .unwrap_or(0);
                                        result.usage = Some(crate::types::Usage {
                                            cache_read_input_tokens: prev_cache_read
                                                + usage.cache_read_input_tokens,
                                            cache_creation_input_tokens: prev_cache_creation
                                                + usage.cache_creation_input_tokens,
                                            ..usage
                                        });
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

    const TEST_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

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

        let (_relay, result_rx) = StreamRelay::start(mock_stream(chunks), TEST_IDLE_TIMEOUT);
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

        let (_relay, result_rx) = StreamRelay::start(mock_stream(chunks), TEST_IDLE_TIMEOUT);
        let result = result_rx.await.unwrap();

        assert_eq!(result.completion_text, "Hello, world!");
    }

    #[tokio::test]
    async fn test_model_capture_from_first_chunk() {
        let chunks = vec![
            sse_line(&make_chunk("gpt-4o-2024-05-13", Some("a"), None)),
            sse_line(&make_chunk("gpt-4o-2024-05-13-v2", Some("b"), None)),
        ];

        let (_relay, result_rx) = StreamRelay::start(mock_stream(chunks), TEST_IDLE_TIMEOUT);
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

        let (_relay, result_rx) = StreamRelay::start(mock_stream(chunks), TEST_IDLE_TIMEOUT);
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

        let (_relay, result_rx) = StreamRelay::start(mock_stream(chunks), TEST_IDLE_TIMEOUT);
        let result = result_rx.await.unwrap();

        // TTFT should be set (non-None) once the first content chunk arrives
        assert!(result.ttft_ms.is_some());
        // The value should be small since we're in a test (likely < 100ms)
        assert!(result.ttft_ms.unwrap() < 1000);
    }

    #[tokio::test]
    async fn test_idle_timeout_sends_error() {
        // Stream that sends one chunk then hangs forever
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, PrismStreamError>>(4);
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let source: Pin<Box<dyn Stream<Item = Result<Bytes, PrismStreamError>> + Send>> =
            Box::pin(stream);

        // Send one chunk, then let the stream idle
        tx.send(Ok(sse_line(&make_chunk("gpt-4o", Some("partial"), None))))
            .await
            .unwrap();

        let (relay, _result_rx) =
            StreamRelay::start(source, Duration::from_millis(100));

        // Drain the relay and collect items
        use futures::StreamExt;
        let items: Vec<_> = relay.collect().await;

        // Should have the first chunk (Ok) and then the timeout error
        assert!(items.len() >= 2, "expected at least 2 items, got {}", items.len());
        assert!(items[0].is_ok());
        let last = items.last().unwrap();
        assert!(last.is_err());
        match last {
            Err(PrismStreamError::Other(msg)) => {
                assert!(msg.contains("idle timeout"), "unexpected error: {msg}");
            }
            other => panic!("expected PrismStreamError::Other, got: {other:?}"),
        }

        // Keep tx alive until after assertions so the stream doesn't end early
        drop(tx);
    }
}
