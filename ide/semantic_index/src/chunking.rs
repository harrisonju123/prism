use sha2::{Digest as _, Sha256};
use std::{path::Path, sync::Arc};

pub struct CodeChunk {
    pub file_path: Arc<Path>,
    pub symbol_name: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    pub content: String,
    /// SHA-256 of the chunk content, used to detect changes.
    pub digest: [u8; 32],
}

/// Approximate character limit per chunk. At ~4 chars/token this is roughly 375 tokens,
/// well under the 8k token limit for text-embedding-3-small.
const MAX_CHUNK_CHARS: usize = 1500;

/// Characters of overlap between consecutive chunks to preserve local context
/// at chunk boundaries.
const OVERLAP_CHARS: usize = 200;

/// Split `content` into fixed-window chunks with overlap.
///
/// Each chunk covers whole lines. The sliding window advances by
/// (MAX_CHUNK_CHARS - OVERLAP_CHARS) characters per step, measured in full lines.
pub fn chunk_text(file_path: Arc<Path>, content: &str) -> Vec<CodeChunk> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < lines.len() {
        let mut end = start;
        let mut chars = 0usize;

        while end < lines.len() {
            chars += lines[end].len() + 1; // +1 for newline
            end += 1;
            if chars >= MAX_CHUNK_CHARS {
                break;
            }
        }

        let chunk_content = lines[start..end].join("\n");
        let digest = sha256_bytes(chunk_content.as_bytes());

        chunks.push(CodeChunk {
            file_path: file_path.clone(),
            symbol_name: None,
            start_line: start as u32,
            end_line: (end - 1) as u32,
            content: chunk_content,
            digest,
        });

        if end >= lines.len() {
            break;
        }

        // Compute overlap: step back OVERLAP_CHARS from the end of this chunk
        let mut overlap_chars = 0usize;
        let mut overlap_start = end;
        while overlap_start > start + 1 {
            overlap_start -= 1;
            overlap_chars += lines[overlap_start].len() + 1;
            if overlap_chars >= OVERLAP_CHARS {
                break;
            }
        }
        // Ensure we always advance at least one line to avoid infinite loops
        start = overlap_start.max(start + 1);
    }

    chunks
}

pub fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Encode f32 embedding as little-endian bytes for SQLite BLOB storage.
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(embedding.len() * 4);
    for f in embedding {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// Decode little-endian BLOB bytes back to f32 embedding.
pub fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_chunk_empty() {
        let path: Arc<Path> = Arc::from(PathBuf::from("test.rs").as_path());
        let chunks = chunk_text(path, "");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_small_file() {
        let path: Arc<Path> = Arc::from(PathBuf::from("test.rs").as_path());
        let content = "fn main() {\n    println!(\"hello\");\n}\n";
        let chunks = chunk_text(path, content);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 0);
    }

    #[test]
    fn test_chunk_large_file() {
        let path: Arc<Path> = Arc::from(PathBuf::from("test.rs").as_path());
        // Generate a file large enough to require multiple chunks
        let line = "x".repeat(100) + "\n";
        let content = line.repeat(50);
        let chunks = chunk_text(path, &content);
        assert!(chunks.len() > 1, "expected multiple chunks for large file");
        // Verify no duplicate coverage - each chunk starts after the previous
        for w in chunks.windows(2) {
            assert!(w[1].start_line > w[0].start_line);
        }
    }

    #[test]
    fn test_embedding_roundtrip() {
        let orig: Vec<f32> = vec![1.0, -0.5, 0.25, 1e-10];
        let bytes = embedding_to_bytes(&orig);
        let decoded = bytes_to_embedding(&bytes);
        for (a, b) in orig.iter().zip(decoded.iter()) {
            assert!((a - b).abs() < 1e-7);
        }
    }
}
