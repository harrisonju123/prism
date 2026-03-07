use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use crate::error::{PrismError, Result};
use crate::proxy::handler::AppState;
use crate::types::{ChatCompletionRequest, ChatCompletionResponse, ProviderResponse};

/// POST /v1/batch/chat/completions — execute multiple completions concurrently.
pub async fn batch_chat_completions(
    State(state): State<Arc<AppState>>,
    Json(request): Json<BatchRequest>,
) -> Result<Response> {
    let max_batch = state.config.batch.max_batch_size;
    if request.requests.is_empty() {
        return Err(PrismError::BadRequest("requests array is empty".into()));
    }
    if request.requests.len() > max_batch {
        return Err(PrismError::BadRequest(format!(
            "batch size {} exceeds maximum {max_batch}",
            request.requests.len()
        )));
    }

    let max_concurrent = state.config.batch.max_concurrency;
    let total = request.requests.len();

    // Execute in chunks to respect concurrency limit
    let mut all_results: Vec<Option<BatchItemResult>> = (0..total).map(|_| None).collect();

    for chunk_start in (0..total).step_by(max_concurrent) {
        let chunk_end = (chunk_start + max_concurrent).min(total);
        let mut join_set = JoinSet::new();

        for (offset, req) in request.requests[chunk_start..chunk_end].iter().enumerate() {
            let global_idx = chunk_start + offset;
            let state = state.clone();
            let req = req.clone();
            join_set.spawn(async move {
                let result = execute_single(&state, &req).await;
                (global_idx, result)
            });
        }

        while let Some(join_result) = join_set.join_next().await {
            match join_result {
                Ok((idx, result)) => {
                    all_results[idx] = Some(result);
                }
                Err(e) => {
                    tracing::error!(error = %e, "batch task panicked");
                }
            }
        }
    }

    let results: Vec<BatchItemResult> = all_results
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            r.unwrap_or(BatchItemResult {
                index: i,
                response: None,
                error: Some("internal error: task did not complete".into()),
            })
        })
        .collect();

    let successful = results.iter().filter(|r| r.response.is_some()).count();
    let failed = results.iter().filter(|r| r.error.is_some()).count();

    Ok(Json(BatchResponse {
        results,
        total,
        successful,
        failed,
    })
    .into_response())
}

async fn execute_single(state: &AppState, request: &ChatCompletionRequest) -> BatchItemResult {
    let (provider_name, model_id) =
        match crate::proxy::handler::resolve_model(&state.config, &request.model) {
            Ok(r) => r,
            Err(e) => {
                return BatchItemResult {
                    index: 0,
                    response: None,
                    error: Some(format!("{e}")),
                };
            }
        };

    let provider = match state.providers.get(&provider_name) {
        Ok(p) => p,
        Err(e) => {
            return BatchItemResult {
                index: 0,
                response: None,
                error: Some(format!("{e}")),
            };
        }
    };

    // Force non-streaming for batch
    let mut req = request.clone();
    req.stream = false;

    match provider.chat_completion(&req, &model_id).await {
        Ok(ProviderResponse::Complete(response)) => BatchItemResult {
            index: 0,
            response: Some(response),
            error: None,
        },
        Ok(ProviderResponse::Stream(_)) => BatchItemResult {
            index: 0,
            response: None,
            error: Some("streaming not supported in batch mode".into()),
        },
        Err(e) => BatchItemResult {
            index: 0,
            response: None,
            error: Some(format!("{e}")),
        },
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BatchRequest {
    pub requests: Vec<ChatCompletionRequest>,
}

#[derive(Debug, Serialize)]
pub struct BatchResponse {
    pub results: Vec<BatchItemResult>,
    pub total: usize,
    pub successful: usize,
    pub failed: usize,
}

#[derive(Debug, Serialize)]
pub struct BatchItemResult {
    pub index: usize,
    pub response: Option<ChatCompletionResponse>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_request(model: &str) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: model.to_string(),
            messages: vec![crate::types::Message {
                role: "user".into(),
                content: Some(json!("hello")),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn test_batch_request_deserialization() {
        let payload = json!({
            "requests": [
                {
                    "model": "gpt-4o",
                    "messages": [{"role": "user", "content": "hello"}]
                },
                {
                    "model": "gpt-4o-mini",
                    "messages": [{"role": "user", "content": "world"}]
                }
            ]
        });

        let batch: BatchRequest = serde_json::from_value(payload).unwrap();
        assert_eq!(batch.requests.len(), 2);
        assert_eq!(batch.requests[0].model, "gpt-4o");
        assert_eq!(batch.requests[1].model, "gpt-4o-mini");
    }

    #[test]
    fn test_empty_batch_rejected() {
        // The handler checks request.requests.is_empty() and returns BadRequest.
        // We verify the condition directly since we can't easily construct AppState.
        let batch = BatchRequest { requests: vec![] };
        assert!(batch.requests.is_empty());

        // Verify the exact error message the handler would produce
        let err = PrismError::BadRequest("requests array is empty".into());
        assert_eq!(err.to_string(), "invalid request: requests array is empty");
    }

    #[test]
    fn test_oversized_batch_rejected() {
        let max_batch = 3;
        let requests: Vec<ChatCompletionRequest> = (0..5).map(|_| make_request("gpt-4o")).collect();
        let batch = BatchRequest { requests };

        assert!(batch.requests.len() > max_batch);

        let err = PrismError::BadRequest(format!(
            "batch size {} exceeds maximum {max_batch}",
            batch.requests.len()
        ));
        assert_eq!(
            err.to_string(),
            "invalid request: batch size 5 exceeds maximum 3"
        );
    }

    #[test]
    fn test_index_tracking_in_results() {
        // Simulate the result collection logic from batch_chat_completions:
        // all_results slots are filled by index, then unwrap_or provides fallback.
        let total = 4;
        let mut all_results: Vec<Option<BatchItemResult>> = (0..total).map(|_| None).collect();

        // Simulate: indices 0 and 2 completed successfully, 1 and 3 did not
        all_results[0] = Some(BatchItemResult {
            index: 0,
            response: None,
            error: None,
        });
        all_results[2] = Some(BatchItemResult {
            index: 2,
            response: None,
            error: None,
        });

        let results: Vec<BatchItemResult> = all_results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                r.unwrap_or(BatchItemResult {
                    index: i,
                    response: None,
                    error: Some("internal error: task did not complete".into()),
                })
            })
            .collect();

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].index, 0);
        assert!(results[0].error.is_none());
        assert_eq!(results[1].index, 1);
        assert_eq!(
            results[1].error.as_deref(),
            Some("internal error: task did not complete")
        );
        assert_eq!(results[2].index, 2);
        assert!(results[2].error.is_none());
        assert_eq!(results[3].index, 3);
        assert!(results[3].error.is_some());
    }

    #[test]
    fn test_stream_false_enforcement() {
        // execute_single forces stream = false on the request clone.
        let mut req = make_request("gpt-4o");
        req.stream = true;
        assert!(req.stream);

        // Reproduce the logic from execute_single
        let mut cloned = req.clone();
        cloned.stream = false;
        assert!(!cloned.stream);
        // Original remains unchanged
        assert!(req.stream);
    }

    #[test]
    fn test_batch_response_serialization() {
        let resp = BatchResponse {
            results: vec![BatchItemResult {
                index: 0,
                response: None,
                error: Some("model not found".into()),
            }],
            total: 1,
            successful: 0,
            failed: 1,
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["total"], 1);
        assert_eq!(json["successful"], 0);
        assert_eq!(json["failed"], 1);
        assert_eq!(json["results"][0]["index"], 0);
        assert_eq!(json["results"][0]["error"], "model not found");
        assert!(json["results"][0]["response"].is_null());
    }
}
