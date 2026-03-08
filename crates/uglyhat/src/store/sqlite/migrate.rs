use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::error::{Error, Result};

const BASELINE_SCHEMA: &str = include_str!("schema.sql");

/// Ordered list of incremental migrations. Index 0 = migration 1.
/// Never modify existing entries — only append.
const MIGRATIONS: &[&str] = &[
    // Migration 1: add current_task_id to agents (pre-versioning DBs may not have it)
    "ALTER TABLE agents ADD COLUMN current_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL",
];

pub fn latest_version() -> i64 {
    MIGRATIONS.len() as i64
}

pub async fn get_version(pool: &SqlitePool) -> Result<i64> {
    let row = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .map_err(|e| Error::Internal(format!("read user_version: {e}")))?;
    Ok(row)
}

async fn set_version(pool: &SqlitePool, version: i64) -> Result<()> {
    // PRAGMA user_version does not support bind parameters
    let sql = format!("PRAGMA user_version = {version}");
    sqlx::query(&sql)
        .execute(pool)
        .await
        .map_err(|e| Error::Internal(format!("set user_version={version}: {e}")))?;
    Ok(())
}

async fn has_tables(pool: &SqlitePool) -> Result<bool> {
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='workspaces'",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| Error::Internal(format!("check sqlite_master: {e}")))?;
    Ok(count > 0)
}

async fn apply_baseline(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(BASELINE_SCHEMA)
        .execute(pool)
        .await
        .map_err(|e| Error::Internal(format!("apply baseline schema: {e}")))?;
    Ok(())
}

async fn run_migration(pool: &SqlitePool, index: usize) -> Result<()> {
    let sql = MIGRATIONS[index];
    let version = (index + 1) as i64;

    if let Err(e) = sqlx::query(sql).execute(pool).await {
        let msg = e.to_string();
        // Gracefully skip "duplicate column" errors — pre-versioning DBs may already have it
        if msg.contains("duplicate column") || msg.contains("already exists") {
            warn!(migration = version, "skipping (column already exists)");
        } else {
            return Err(Error::Internal(format!("migration {version}: {e}")));
        }
    }
    Ok(())
}

pub(crate) async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    let current = get_version(pool).await?;
    let latest = latest_version();

    if current == 0 {
        if !has_tables(pool).await? {
            // Fresh database — apply full baseline, then jump to latest
            info!("fresh database — applying baseline schema");
            apply_baseline(pool).await?;
            set_version(pool, latest).await?;
            info!(version = latest, "schema up to date");
        } else {
            // Pre-versioning database — baseline is idempotent, then run all migrations
            info!("pre-versioning database — running all migrations");
            apply_baseline(pool).await?;
            for i in 0..MIGRATIONS.len() {
                let v = (i + 1) as i64;
                info!(migration = v, "applying migration");
                run_migration(pool, i).await?;
                set_version(pool, v).await?;
            }
            info!(version = latest, "schema up to date");
        }
    } else if current < latest {
        // Versioned but behind — run only new migrations
        info!(
            from = current,
            to = latest,
            "running incremental migrations"
        );
        for i in (current as usize)..MIGRATIONS.len() {
            let v = (i + 1) as i64;
            info!(migration = v, "applying migration");
            run_migration(pool, i).await?;
            set_version(pool, v).await?;
        }
        info!(version = latest, "schema up to date");
    } else {
        info!(version = current, "schema up to date");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    async fn mem_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true)
            .foreign_keys(true);
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .expect("open memory pool")
    }

    #[tokio::test]
    async fn fresh_database_gets_latest_version() {
        let pool = mem_pool().await;
        run_migrations(&pool).await.expect("migrations failed");
        let v = get_version(&pool).await.expect("get_version failed");
        assert_eq!(v, latest_version());
    }

    #[tokio::test]
    async fn pre_versioning_database_upgrades() {
        let pool = mem_pool().await;

        // Apply baseline schema without current_task_id to simulate pre-versioning DB
        let schema_without_col = BASELINE_SCHEMA.replace(
            "    current_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,\n",
            "",
        );
        sqlx::raw_sql(&schema_without_col)
            .execute(&pool)
            .await
            .expect("apply reduced schema");

        // user_version stays 0 (pre-versioning)
        assert_eq!(get_version(&pool).await.unwrap(), 0);
        assert!(has_tables(&pool).await.unwrap());

        run_migrations(&pool).await.expect("migrations failed");

        let v = get_version(&pool).await.unwrap();
        assert_eq!(v, latest_version());

        // Verify the column was added
        let col_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pragma_table_info('agents') WHERE name='current_task_id'",
        )
        .fetch_one(&pool)
        .await
        .expect("pragma query");
        assert_eq!(col_count, 1, "current_task_id column should exist");
    }

    #[tokio::test]
    async fn already_versioned_database_is_idempotent() {
        let pool = mem_pool().await;

        run_migrations(&pool).await.expect("first run failed");
        let v1 = get_version(&pool).await.unwrap();

        run_migrations(&pool).await.expect("second run failed");
        let v2 = get_version(&pool).await.unwrap();

        assert_eq!(v1, v2);
        assert_eq!(v1, latest_version());
    }
}
