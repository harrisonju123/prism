use crate::{
    chunking::bytes_to_embedding,
    db::{fetch_chunk_embeddings, fetch_chunks_by_ids},
    embedding::EmbeddingProvider,
};
use anyhow::Result;
use sqlez::thread_safe_connection::ThreadSafeConnection;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file_path: PathBuf,
    pub symbol_name: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    pub score: f32,
}

/// Embed `query`, rank all stored chunks by cosine similarity, then load full metadata
/// only for the top `limit` results.
///
/// Two-phase loading avoids pulling file paths / symbol names for every chunk in the repo
/// (which adds up fast for large codebases). Only (id, embedding) is loaded in the hot
/// path; a targeted follow-up query fetches metadata for the small top-k set.
pub async fn search(
    db: &ThreadSafeConnection,
    provider: &dyn EmbeddingProvider,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let query_embeddings = provider.embed(&[query]).await?;
    let query_vec = query_embeddings
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("embedding provider returned no vectors for query"))?;

    // Phase 1: load only ids + embedding blobs
    let id_embeddings = fetch_chunk_embeddings(db)?;
    if id_embeddings.is_empty() {
        return Ok(Vec::new());
    }

    let mut scored: Vec<(f32, i64)> = id_embeddings
        .iter()
        .map(|(id, blob)| {
            let embedding = bytes_to_embedding(blob);
            let score = cosine_similarity(&query_vec, &embedding);
            (score, *id)
        })
        .collect();

    // Partial sort: bring top `limit` entries to front, then final sort on that small slice
    let n = scored.len();
    if n > 0 {
        let k = limit.saturating_sub(1).min(n - 1);
        scored.select_nth_unstable_by(k, |a, b| {
            b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    scored.truncate(limit);
    scored.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // Phase 2: fetch metadata only for the top-k ids
    let top_ids: Vec<i64> = scored.iter().map(|(_, id)| *id).collect();
    let metadata = fetch_chunks_by_ids(db, &top_ids)?;
    let metadata_map: std::collections::HashMap<i64, _> =
        metadata.into_iter().map(|c| (c.id, c)).collect();

    let results = scored
        .into_iter()
        .filter_map(|(score, id)| {
            let chunk = metadata_map.get(&id)?;
            Some(SearchResult {
                file_path: PathBuf::from(&chunk.file_path),
                symbol_name: chunk.symbol_name.clone(),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                score,
            })
        })
        .collect();

    Ok(results)
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::cosine_similarity;

    #[test]
    fn test_identical_vectors() {
        let v = vec![1.0_f32, 0.5, -0.5];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_orthogonal_vectors() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn test_mismatched_lengths() {
        let a = vec![1.0_f32, 2.0];
        let b = vec![1.0_f32];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }
}
