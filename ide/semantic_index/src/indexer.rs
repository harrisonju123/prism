use crate::{
    chunking::{chunk_text, embedding_to_bytes, sha256_bytes},
    db::{delete_file, delete_stale_chunks, upsert_chunk, upsert_file},
    embedding::EmbeddingProvider,
};
use anyhow::Result;
use sqlez::thread_safe_connection::ThreadSafeConnection;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use walkdir::WalkDir;

/// Per-project indexing settings.
pub struct IndexSettings {
    /// Skip files larger than this many bytes.
    pub max_file_bytes: u64,
    /// Embed this many chunk texts per API call.
    pub batch_size: usize,
}

impl Default for IndexSettings {
    fn default() -> Self {
        Self { max_file_bytes: 1024 * 1024, batch_size: 100 }
    }
}

/// Summary returned after indexing a worktree directory.
#[derive(Debug, Default)]
pub struct IndexStats {
    pub files_scanned: usize,
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub chunks_embedded: usize,
}

/// Index all indexable files under `root_path`, assigning them to `worktree_id` in the DB.
///
/// Files whose mtime and SHA-256 haven't changed since the last index run are skipped.
/// File deletions are handled by the caller (subscribe to worktree events and call
/// `delete_file` when entries are removed).
pub async fn index_directory(
    db: &ThreadSafeConnection,
    provider: &dyn EmbeddingProvider,
    worktree_id: i64,
    root_path: &Path,
    settings: &IndexSettings,
) -> Result<IndexStats> {
    let paths = collect_indexable_files(root_path, settings.max_file_bytes);
    let mut stats = IndexStats { files_scanned: paths.len(), ..Default::default() };

    for abs_path in paths {
        match index_file(db, provider, worktree_id, root_path, &abs_path, settings).await {
            Ok(indexed) => {
                if indexed {
                    stats.files_indexed += 1;
                } else {
                    stats.files_skipped += 1;
                }
            }
            Err(err) => {
                log::warn!("semantic_index: skipping {:?}: {err}", abs_path);
                stats.files_skipped += 1;
            }
        }
    }

    Ok(stats)
}

/// Index a single file. Returns `true` if the file was (re-)indexed, `false` if it was
/// unchanged and skipped.
pub async fn index_file(
    db: &ThreadSafeConnection,
    provider: &dyn EmbeddingProvider,
    worktree_id: i64,
    root_path: &Path,
    abs_path: &Path,
    settings: &IndexSettings,
) -> Result<bool> {
    let relative_path = abs_path
        .strip_prefix(root_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| abs_path.to_string_lossy().into_owned());

    let meta = std::fs::metadata(abs_path)?;
    let (mtime_secs, mtime_nanos) = system_time_to_secs_nanos(meta.modified()?);

    let content = std::fs::read(abs_path)?;
    if content.len() as u64 > settings.max_file_bytes {
        return Ok(false);
    }

    let content_sha = sha256_bytes(&content);

    // Check if this file needs re-indexing
    if let Some((stored_secs, stored_nanos, stored_sha)) =
        crate::db::get_file_mtime_and_sha(db, worktree_id, &relative_path)?
    {
        if stored_secs == mtime_secs
            && stored_nanos == mtime_nanos
            && stored_sha.as_slice() == content_sha.as_slice()
        {
            return Ok(false);
        }
    }

    let text = match String::from_utf8(content) {
        Ok(s) => s,
        Err(_) => return Ok(false), // skip binary files
    };

    let file_path: Arc<Path> = Arc::from(abs_path);
    let chunks = chunk_text(file_path, &text);
    if chunks.is_empty() {
        return Ok(false);
    }

    let file_id = upsert_file(
        db,
        worktree_id,
        relative_path,
        mtime_secs,
        mtime_nanos,
        content_sha.to_vec(),
    )
    .await?;

    // Embed in batches
    let chunk_texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(chunks.len());

    for batch in chunk_texts.chunks(settings.batch_size) {
        let embeddings = provider.embed(batch).await?;
        all_embeddings.extend(embeddings);
    }

    anyhow::ensure!(
        all_embeddings.len() == chunks.len(),
        "embedding provider returned {} vectors for {} chunks",
        all_embeddings.len(),
        chunks.len()
    );

    let current_ranges: Vec<(u32, u32)> =
        chunks.iter().map(|c| (c.start_line, c.end_line)).collect();

    for (chunk, embedding) in chunks.iter().zip(all_embeddings.iter()) {
        upsert_chunk(
            db,
            file_id,
            chunk.symbol_name.clone(),
            chunk.start_line,
            chunk.end_line,
            chunk.digest.to_vec(),
            embedding_to_bytes(embedding),
        )
        .await?;
    }

    delete_stale_chunks(db, file_id, current_ranges).await?;

    Ok(true)
}

/// Remove a file and all its chunks from the index (called when the file is deleted).
pub async fn remove_file(
    db: &ThreadSafeConnection,
    worktree_id: i64,
    relative_path: String,
) -> Result<()> {
    delete_file(db, worktree_id, relative_path).await
}

/// Extensions considered indexable (text-based source code and prose).
static INDEXABLE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "c", "cpp", "cc", "h", "hpp", "hh",
    "cs", "rb", "swift", "kt", "kts", "scala", "clj", "cljs", "ex", "exs", "elm", "hs", "ml",
    "mli", "fs", "fsi", "fsx", "lua", "php", "sh", "bash", "zsh", "fish", "md", "mdx", "txt",
    "toml", "yaml", "yml", "json", "graphql", "gql", "sql", "proto", "xml", "html", "css",
    "scss", "less",
];

fn is_indexable(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| INDEXABLE_EXTENSIONS.contains(&e))
        .unwrap_or(false)
}

fn collect_indexable_files(root: &Path, max_bytes: u64) -> Vec<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            // Skip hidden and well-known non-source directories
            e.file_name()
                .to_str()
                .map(|name| !name.starts_with('.') && name != "node_modules" && name != "target")
                .unwrap_or(true)
        })
        .filter_map(|result| {
            match result {
                Ok(entry) if entry.file_type().is_file() && is_indexable(entry.path()) => {
                    let meta = entry.metadata().ok()?;
                    (meta.len() <= max_bytes).then(|| entry.into_path())
                }
                Ok(_) => None,
                Err(e) => {
                    log::warn!("semantic_index: directory walk error: {e}");
                    None
                }
            }
        })
        .collect()
}

fn system_time_to_secs_nanos(t: SystemTime) -> (i64, i32) {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos() as i32),
        Err(_) => (0, 0),
    }
}
