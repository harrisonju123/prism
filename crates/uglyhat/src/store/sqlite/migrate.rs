use sqlx::SqlitePool;
use tracing::info;

use crate::error::{Error, Result};

const BASELINE_SCHEMA: &str = include_str!("schema.sql");

/// No incremental migrations yet — this is a fresh v2 schema.
const MIGRATIONS: &[&str] = &[];

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

pub(crate) async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    let current = get_version(pool).await?;
    let latest = latest_version();

    if current == 0 {
        if !has_tables(pool).await? {
            info!("fresh database — applying baseline schema");
            apply_baseline(pool).await?;
            set_version(pool, latest).await?;
            info!(version = latest, "schema up to date");
        } else {
            // Existing tables from v1 — this is a breaking change, fresh DB required
            info!("existing database detected — applying v2 baseline (additive)");
            apply_baseline(pool).await?;
            set_version(pool, latest).await?;
            info!(version = latest, "schema up to date");
        }
    } else if current < latest {
        info!(
            from = current,
            to = latest,
            "running incremental migrations"
        );
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let v = (i + 1) as i64;
            info!(migration = v, "applying migration");
            sqlx::query(sql)
                .execute(pool)
                .await
                .map_err(|e| Error::Internal(format!("migration {v}: {e}")))?;
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
