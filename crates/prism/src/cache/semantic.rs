use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::types::ChatCompletionResponse;

/// A semantic cache entry: embedding vector + response + metadata.
#[derive(Debug, Clone)]
struct SemanticEntry {
    embedding: Vec<f32>,
    response: ChatCompletionResponse,
    model: String,
    inserted_at: Instant,
}

/// Semantic cache that finds similar prompts by cosine similarity.
///
/// Two-tier: exact hash lookup first (handled by `ResponseCache`),
/// then semantic similarity via this cache.
pub struct SemanticCache {
    entries: DashMap<String, SemanticEntry>,
    max_size: usize,
    ttl: Duration,
    similarity_threshold: f32,
}

impl SemanticCache {
    pub fn new(max_size: usize, ttl_secs: u64, similarity_threshold: f32) -> Self {
        Self {
            entries: DashMap::new(),
            max_size,
            ttl: Duration::from_secs(ttl_secs),
            similarity_threshold,
        }
    }

    /// Search for a semantically similar cached response.
    /// Returns the response and similarity score if found above threshold.
    pub fn search(
        &self,
        query_embedding: &[f32],
        model: &str,
    ) -> Option<(ChatCompletionResponse, f32)> {
        let now = Instant::now();
        let mut best_match: Option<(ChatCompletionResponse, f32)> = None;

        for entry in self.entries.iter() {
            let e = entry.value();

            // Skip expired entries
            if now.duration_since(e.inserted_at) > self.ttl {
                continue;
            }

            // Only match same model
            if e.model != model {
                continue;
            }

            let similarity = cosine_similarity(query_embedding, &e.embedding);
            if similarity >= self.similarity_threshold {
                if best_match
                    .as_ref()
                    .is_none_or(|(_, best_sim)| similarity > *best_sim)
                {
                    best_match = Some((e.response.clone(), similarity));
                }
            }
        }

        best_match
    }

    /// Insert a prompt embedding + response into the semantic cache.
    pub fn insert(
        &self,
        cache_key: String,
        embedding: Vec<f32>,
        model: String,
        response: ChatCompletionResponse,
    ) {
        if self.entries.len() >= self.max_size {
            self.evict_oldest();
        }

        self.entries.insert(
            cache_key,
            SemanticEntry {
                embedding,
                response,
                model,
                inserted_at: Instant::now(),
            },
        );
    }

    /// Remove expired entries.
    pub fn prune_expired(&self) {
        let now = Instant::now();
        let ttl = self.ttl;
        self.entries
            .retain(|_, entry| now.duration_since(entry.inserted_at) <= ttl);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn evict_oldest(&self) {
        let mut oldest_key: Option<String> = None;
        let mut oldest_time = Instant::now();

        for entry in self.entries.iter() {
            if entry.value().inserted_at < oldest_time {
                oldest_time = entry.value().inserted_at;
                oldest_key = Some(entry.key().clone());
            }
        }

        if let Some(key) = oldest_key {
            self.entries.remove(&key);
        }
    }
}

/// Compute cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < f32::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

/// Compute a simple bag-of-words embedding from text.
/// This is a lightweight fallback when no external embedding service is available.
/// Uses character trigram hashing to create a fixed-size vector.
pub fn simple_text_embedding(text: &str, dim: usize) -> Vec<f32> {
    let mut embedding = vec![0.0_f32; dim];
    let text_lower = text.to_lowercase();
    let chars: Vec<char> = text_lower.chars().collect();

    for window in chars.windows(3) {
        let hash = simple_hash(window) % dim;
        embedding[hash] += 1.0;
    }

    // L2 normalize
    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in &mut embedding {
            *x /= norm;
        }
    }

    embedding
}

fn simple_hash(chars: &[char]) -> usize {
    let mut hash = 5381_usize;
    for c in chars {
        hash = hash.wrapping_mul(33).wrapping_add(*c as usize);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatCompletionResponse, Choice, Message, Usage};

    fn make_response(text: &str) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "test".into(),
            object: "chat.completion".into(),
            created: 0,
            model: "test-model".into(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".into(),
                    content: Some(serde_json::Value::String(text.into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: serde_json::Map::new(),
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(Usage::default()),
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 0.001);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < 0.001);
    }

    #[test]
    fn cosine_similarity_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn simple_embedding_deterministic() {
        let e1 = simple_text_embedding("hello world", 64);
        let e2 = simple_text_embedding("hello world", 64);
        assert_eq!(e1, e2);
    }

    #[test]
    fn similar_texts_high_similarity() {
        let e1 = simple_text_embedding("what is the capital of France?", 128);
        let e2 = simple_text_embedding("what is the capital of France", 128);
        let sim = cosine_similarity(&e1, &e2);
        assert!(
            sim > 0.9,
            "similar texts should have high similarity, got {sim}"
        );
    }

    #[test]
    fn different_texts_lower_similarity() {
        let e1 = simple_text_embedding("what is the capital of France?", 128);
        let e2 = simple_text_embedding("how to make chocolate cake", 128);
        let sim = cosine_similarity(&e1, &e2);
        assert!(
            sim < 0.8,
            "different texts should have lower similarity, got {sim}"
        );
    }

    #[test]
    fn semantic_cache_hit() {
        let cache = SemanticCache::new(100, 3600, 0.9);
        let emb = simple_text_embedding("what is the capital of France?", 128);
        cache.insert(
            "key1".into(),
            emb.clone(),
            "gpt-4o".into(),
            make_response("Paris"),
        );

        let query = simple_text_embedding("what is the capital of France", 128);
        let result = cache.search(&query, "gpt-4o");
        assert!(result.is_some(), "should find similar entry");
    }

    #[test]
    fn semantic_cache_miss_different_model() {
        let cache = SemanticCache::new(100, 3600, 0.9);
        let emb = simple_text_embedding("what is the capital of France?", 128);
        cache.insert(
            "key1".into(),
            emb.clone(),
            "gpt-4o".into(),
            make_response("Paris"),
        );

        let query = simple_text_embedding("what is the capital of France?", 128);
        let result = cache.search(&query, "claude-sonnet-4");
        assert!(result.is_none(), "should not match different model");
    }

    #[test]
    fn semantic_cache_miss_below_threshold() {
        let cache = SemanticCache::new(100, 3600, 0.99);
        let emb = simple_text_embedding("hello world", 128);
        cache.insert(
            "key1".into(),
            emb.clone(),
            "gpt-4o".into(),
            make_response("response"),
        );

        let query = simple_text_embedding("completely different query about programming", 128);
        let result = cache.search(&query, "gpt-4o");
        assert!(result.is_none(), "should not match dissimilar text");
    }
}
