use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

#[cfg(feature = "postgres")]
use sqlx::PgPool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelAlias {
    pub id: Uuid,
    pub name: String,
    pub target_model: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// In-memory cache for model alias lookups.
pub struct AliasCache {
    inner: RwLock<HashMap<String, String>>,
}

impl AliasCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(HashMap::new()),
        })
    }

    pub async fn get(&self, name: &str) -> Option<String> {
        self.inner.read().await.get(name).cloned()
    }

    pub async fn set(&self, name: String, target: String) {
        self.inner.write().await.insert(name, target);
    }

    pub async fn remove(&self, name: &str) {
        self.inner.write().await.remove(name);
    }

    pub async fn load_all(&self, aliases: impl IntoIterator<Item = (String, String)>) {
        let mut inner = self.inner.write().await;
        inner.clear();
        for (k, v) in aliases {
            inner.insert(k, v);
        }
    }
}

#[cfg(feature = "postgres")]
pub struct AliasRepository {
    pool: PgPool,
}

#[cfg(feature = "postgres")]
impl AliasRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        name: &str,
        target_model: &str,
        description: Option<&str>,
    ) -> sqlx::Result<ModelAlias> {
        sqlx::query_as::<_, AliasRow>(
            r#"INSERT INTO model_aliases (name, target_model, description)
               VALUES ($1, $2, $3)
               RETURNING id, name, target_model, description, created_at, updated_at"#,
        )
        .bind(name)
        .bind(target_model)
        .bind(description)
        .fetch_one(&self.pool)
        .await
        .map(ModelAlias::from)
    }

    pub async fn list(&self) -> sqlx::Result<Vec<ModelAlias>> {
        sqlx::query_as::<_, AliasRow>(
            r#"SELECT id, name, target_model, description, created_at, updated_at
               FROM model_aliases
               ORDER BY name ASC"#,
        )
        .fetch_all(&self.pool)
        .await
        .map(|rows| rows.into_iter().map(ModelAlias::from).collect())
    }

    pub async fn get(&self, name: &str) -> sqlx::Result<Option<ModelAlias>> {
        sqlx::query_as::<_, AliasRow>(
            r#"SELECT id, name, target_model, description, created_at, updated_at
               FROM model_aliases WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map(|opt| opt.map(ModelAlias::from))
    }

    pub async fn update(
        &self,
        name: &str,
        target_model: Option<&str>,
        description: Option<Option<&str>>,
    ) -> sqlx::Result<Option<ModelAlias>> {
        sqlx::query_as::<_, AliasRow>(
            r#"UPDATE model_aliases SET
               target_model = COALESCE($2, target_model),
               description  = CASE WHEN $3 THEN $4 ELSE description END,
               updated_at   = NOW()
               WHERE name = $1
               RETURNING id, name, target_model, description, created_at, updated_at"#,
        )
        .bind(name)
        .bind(target_model)
        .bind(description.is_some())
        .bind(description.flatten())
        .fetch_optional(&self.pool)
        .await
        .map(|opt| opt.map(ModelAlias::from))
    }

    pub async fn delete(&self, name: &str) -> sqlx::Result<bool> {
        let result = sqlx::query("DELETE FROM model_aliases WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Load all aliases as (name, target_model) pairs.
    pub async fn load_all_pairs(&self) -> sqlx::Result<Vec<(String, String)>> {
        let rows = self.list().await?;
        Ok(rows.into_iter().map(|a| (a.name, a.target_model)).collect())
    }
}

#[cfg(feature = "postgres")]
#[derive(sqlx::FromRow)]
struct AliasRow {
    id: Uuid,
    name: String,
    target_model: String,
    description: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
impl From<AliasRow> for ModelAlias {
    fn from(r: AliasRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            target_model: r.target_model,
            description: r.description,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cache_get_miss() {
        let c = AliasCache::new();
        assert_eq!(c.get("nonexistent").await, None);
    }

    #[tokio::test]
    async fn cache_set_then_get() {
        let c = AliasCache::new();
        c.set("fast".into(), "gpt-4o-mini".into()).await;
        assert_eq!(c.get("fast").await, Some("gpt-4o-mini".into()));
    }

    #[tokio::test]
    async fn cache_remove() {
        let c = AliasCache::new();
        c.set("k".into(), "v".into()).await;
        c.remove("k").await;
        assert_eq!(c.get("k").await, None);
    }

    #[tokio::test]
    async fn cache_load_all_replaces() {
        let c = AliasCache::new();
        c.set("old".into(), "old-target".into()).await;
        c.load_all([
            ("a".into(), "model-a".into()),
            ("b".into(), "model-b".into()),
        ])
        .await;
        assert_eq!(c.get("old").await, None);
        assert_eq!(c.get("a").await, Some("model-a".into()));
        assert_eq!(c.get("b").await, Some("model-b".into()));
    }

    #[tokio::test]
    async fn cache_overwrite() {
        let c = AliasCache::new();
        c.set("key".into(), "first".into()).await;
        c.set("key".into(), "second".into()).await;
        assert_eq!(c.get("key").await, Some("second".into()));
    }
}

/// Stub for non-Postgres builds.
#[cfg(not(feature = "postgres"))]
pub struct AliasRepository;

#[cfg(not(feature = "postgres"))]
impl AliasRepository {
    pub fn new_stub() -> Self {
        Self
    }
}
