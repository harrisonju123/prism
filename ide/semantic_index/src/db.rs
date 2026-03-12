use anyhow::Result;
use sqlez::{domain::Domain, thread_safe_connection::ThreadSafeConnection};
use sqlez_macros::sql;

pub struct SemanticIndexDb;

impl Domain for SemanticIndexDb {
    const NAME: &str = stringify!(SemanticIndexDb);
    const MIGRATIONS: &[&str] = &[sql!(
        CREATE TABLE files (
            id INTEGER PRIMARY KEY,
            worktree_id INTEGER NOT NULL,
            relative_path TEXT NOT NULL,
            mtime_secs INTEGER NOT NULL,
            mtime_nanos INTEGER NOT NULL,
            content_sha256 BLOB NOT NULL,
            UNIQUE(worktree_id, relative_path)
        );
        CREATE TABLE chunks (
            id INTEGER PRIMARY KEY,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            symbol_name TEXT,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            content_sha256 BLOB NOT NULL,
            embedding BLOB NOT NULL,
            UNIQUE(file_id, start_line, end_line)
        );
    )];
}

/// A row from the chunks + files join, used for similarity search.
pub struct StoredChunk {
    pub id: i64,
    pub file_path: String,
    pub symbol_name: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
}

/// Insert or update a file record, returning its row id.
pub async fn upsert_file(
    db: &ThreadSafeConnection,
    worktree_id: i64,
    relative_path: String,
    mtime_secs: i64,
    mtime_nanos: i32,
    content_sha256: Vec<u8>,
) -> Result<i64> {
    db.write(move |c| -> Result<i64> {
        let mut upsert = c.exec_bound(
            "INSERT INTO files (worktree_id, relative_path, mtime_secs, mtime_nanos, content_sha256)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(worktree_id, relative_path)
             DO UPDATE SET mtime_secs=excluded.mtime_secs,
                           mtime_nanos=excluded.mtime_nanos,
                           content_sha256=excluded.content_sha256",
        )?;
        upsert((worktree_id, relative_path.clone(), mtime_secs, mtime_nanos, content_sha256))?;

        let mut get_id = c.select_row_bound::<(i64, String), i64>(
            "SELECT id FROM files WHERE worktree_id=? AND relative_path=?",
        )?;
        get_id((worktree_id, relative_path))?.ok_or_else(|| anyhow::anyhow!("file not found after upsert"))
    })
    .await
}

/// Delete all chunks for a file (e.g. when the file was deleted from the worktree).
pub async fn delete_file(
    db: &ThreadSafeConnection,
    worktree_id: i64,
    relative_path: String,
) -> Result<()> {
    db.write(move |c| -> Result<()> {
        let mut del = c.exec_bound(
            "DELETE FROM files WHERE worktree_id=? AND relative_path=?",
        )?;
        del((worktree_id, relative_path))?;
        Ok(())
    })
    .await
}

/// Look up the mtime and sha256 of a previously-indexed file.
/// Returns `None` if the file has not been indexed yet.
pub fn get_file_mtime_and_sha(
    db: &ThreadSafeConnection,
    worktree_id: i64,
    relative_path: &str,
) -> Result<Option<(i64, i32, Vec<u8>)>> {
    db.select_row_bound::<(i64, String), (i64, i32, Vec<u8>)>(
        "SELECT mtime_secs, mtime_nanos, content_sha256 FROM files WHERE worktree_id=? AND relative_path=?",
    )?((worktree_id, relative_path.to_string()))
}

/// Upsert one chunk record with its embedding blob.
pub async fn upsert_chunk(
    db: &ThreadSafeConnection,
    file_id: i64,
    symbol_name: Option<String>,
    start_line: u32,
    end_line: u32,
    content_sha256: Vec<u8>,
    embedding: Vec<u8>,
) -> Result<()> {
    db.write(move |c| -> Result<()> {
        let mut insert = c.exec_bound(
            "INSERT INTO chunks (file_id, symbol_name, start_line, end_line, content_sha256, embedding)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(file_id, start_line, end_line)
             DO UPDATE SET symbol_name=excluded.symbol_name,
                           content_sha256=excluded.content_sha256,
                           embedding=excluded.embedding",
        )?;
        insert((file_id, symbol_name, start_line, end_line, content_sha256, embedding))?;
        Ok(())
    })
    .await
}

/// Delete stale chunks for a file that no longer match any of the current chunk ranges.
pub async fn delete_stale_chunks(
    db: &ThreadSafeConnection,
    file_id: i64,
    current_ranges: Vec<(u32, u32)>,
) -> Result<()> {
    if current_ranges.is_empty() {
        return db.write(move |c| -> Result<()> {
            let mut del = c.exec_bound("DELETE FROM chunks WHERE file_id=?")?;
            del(file_id)?;
            Ok(())
        })
        .await;
    }

    // Fetch all existing chunk ids and ranges, then delete the ones not in current_ranges
    let existing: Vec<(i64, u32, u32)> = db
        .select_bound::<i64, (i64, u32, u32)>(
            "SELECT id, start_line, end_line FROM chunks WHERE file_id=?",
        )?
        (file_id)?;

    let stale_ids: Vec<i64> = existing
        .into_iter()
        .filter(|(_, s, e)| !current_ranges.contains(&(*s, *e)))
        .map(|(id, _, _)| id)
        .collect();

    if stale_ids.is_empty() {
        return Ok(());
    }

    // Build a single DELETE with inline ids — safe because these are i64 DB primary keys
    db.write(move |c| -> Result<()> {
        let ids_csv: String = stale_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM chunks WHERE id IN ({ids_csv})");
        c.exec(&sql)?()?;
        Ok(())
    })
    .await
}

/// Load only (id, embedding) for every chunk — used in the similarity ranking hot path.
/// Avoids loading file paths and symbol names until we know which chunks are relevant.
pub fn fetch_chunk_embeddings(db: &ThreadSafeConnection) -> Result<Vec<(i64, Vec<u8>)>> {
    db.select::<(i64, Vec<u8>)>("SELECT id, embedding FROM chunks")?()
}

/// Load full metadata for a specific set of chunk ids, in id order.
pub fn fetch_chunks_by_ids(db: &ThreadSafeConnection, ids: &[i64]) -> Result<Vec<StoredChunk>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids_csv: String = ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT c.id, f.relative_path, c.symbol_name, c.start_line, c.end_line
         FROM chunks c JOIN files f ON c.file_id = f.id
         WHERE c.id IN ({ids_csv})"
    );
    let rows = db.select::<(i64, String, Option<String>, u32, u32)>(&sql)?()?;
    Ok(rows
        .into_iter()
        .map(|(id, file_path, symbol_name, start_line, end_line)| StoredChunk {
            id,
            file_path,
            symbol_name,
            start_line,
            end_line,
        })
        .collect())
}
