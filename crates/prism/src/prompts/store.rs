use std::collections::HashMap;
use std::sync::RwLock;

use chrono::Utc;
use uuid::Uuid;

use super::types::PromptTemplate;

// ---------------------------------------------------------------------------
// PromptStore — dispatches to InMemory or Postgres backend
// ---------------------------------------------------------------------------

pub enum PromptStore {
    InMemory(InMemoryPromptStore),
    #[cfg(feature = "postgres")]
    Postgres(PostgresPromptStore),
}

impl PromptStore {
    pub fn new() -> Self {
        Self::InMemory(InMemoryPromptStore::new())
    }

    #[cfg(feature = "postgres")]
    pub fn new_postgres(pool: sqlx::PgPool) -> Self {
        Self::Postgres(PostgresPromptStore::new(pool))
    }

    pub fn create(
        &self,
        name: &str,
        content: &str,
        model_hint: Option<String>,
        metadata: serde_json::Value,
    ) -> PromptTemplate {
        match self {
            Self::InMemory(store) => store.create(name, content, model_hint, metadata),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                // Sync create for Postgres uses a blocking fallback
                // In practice, use create_async
                InMemoryPromptStore::new().create(name, content, model_hint, metadata)
            }
        }
    }

    pub fn get_latest(&self, name: &str) -> Option<PromptTemplate> {
        match self {
            Self::InMemory(store) => store.get_latest(name),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => None, // Use get_latest_async
        }
    }

    pub fn get_version(&self, name: &str, version: u32) -> Option<PromptTemplate> {
        match self {
            Self::InMemory(store) => store.get_version(name, version),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => None,
        }
    }

    pub fn list(&self) -> Vec<PromptTemplate> {
        match self {
            Self::InMemory(store) => store.list(),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => Vec::new(),
        }
    }

    // --- Async Postgres operations ---

    pub async fn create_async(
        &self,
        name: &str,
        content: &str,
        model_hint: Option<String>,
        metadata: serde_json::Value,
    ) -> Result<PromptTemplate, String> {
        match self {
            Self::InMemory(store) => Ok(store.create(name, content, model_hint, metadata)),
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.create(name, content, model_hint, metadata).await,
        }
    }

    pub async fn get_latest_async(&self, name: &str) -> Option<PromptTemplate> {
        match self {
            Self::InMemory(store) => store.get_latest(name),
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.get_latest(name).await,
        }
    }

    pub async fn get_version_async(&self, name: &str, version: u32) -> Option<PromptTemplate> {
        match self {
            Self::InMemory(store) => store.get_version(name, version),
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.get_version(name, version).await,
        }
    }

    pub async fn list_async(&self) -> Vec<PromptTemplate> {
        match self {
            Self::InMemory(store) => store.list(),
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.list().await,
        }
    }

    pub async fn get_versions_async(&self, name: &str) -> Vec<PromptTemplate> {
        match self {
            Self::InMemory(store) => store.get_all_versions(name),
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.get_versions(name).await,
        }
    }

    pub async fn rollback_async(&self, name: &str, version: u32) -> Result<PromptTemplate, String> {
        match self {
            Self::InMemory(store) => store
                .get_version(name, version)
                .ok_or_else(|| format!("version {version} not found for '{name}'")),
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.rollback(name, version).await,
        }
    }
}

// ---------------------------------------------------------------------------
// In-memory backend
// ---------------------------------------------------------------------------

pub struct InMemoryPromptStore {
    templates: RwLock<HashMap<String, Vec<PromptTemplate>>>,
}

impl InMemoryPromptStore {
    pub fn new() -> Self {
        Self {
            templates: RwLock::new(HashMap::new()),
        }
    }

    pub fn create(
        &self,
        name: &str,
        content: &str,
        model_hint: Option<String>,
        metadata: serde_json::Value,
    ) -> PromptTemplate {
        let mut store = self.templates.write().unwrap();
        let versions = store.entry(name.to_string()).or_default();

        let version = versions.last().map(|v| v.version + 1).unwrap_or(1);
        let template = PromptTemplate {
            id: Uuid::new_v4(),
            name: name.to_string(),
            version,
            content: content.to_string(),
            model_hint,
            metadata,
            created_at: Utc::now(),
            active: true,
        };

        versions.push(template.clone());
        template
    }

    pub fn get_latest(&self, name: &str) -> Option<PromptTemplate> {
        let store = self.templates.read().unwrap();
        store
            .get(name)
            .and_then(|versions| versions.iter().rev().find(|v| v.active).cloned())
    }

    pub fn get_version(&self, name: &str, version: u32) -> Option<PromptTemplate> {
        let store = self.templates.read().unwrap();
        store
            .get(name)
            .and_then(|versions| versions.iter().find(|v| v.version == version).cloned())
    }

    pub fn get_all_versions(&self, name: &str) -> Vec<PromptTemplate> {
        let store = self.templates.read().unwrap();
        store.get(name).cloned().unwrap_or_default()
    }

    pub fn list(&self) -> Vec<PromptTemplate> {
        let store = self.templates.read().unwrap();
        store
            .values()
            .filter_map(|versions| versions.iter().rev().find(|v| v.active).cloned())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Postgres backend
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
pub struct PostgresPromptStore {
    pool: sqlx::PgPool,
}

#[cfg(feature = "postgres")]
impl PostgresPromptStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        name: &str,
        content: &str,
        model_hint: Option<String>,
        metadata: serde_json::Value,
    ) -> Result<PromptTemplate, String> {
        // Get next version number
        let next_version: i32 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM prompt_templates WHERE name = $1",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("failed to get next version: {e}"))?;

        let row = sqlx::query_as::<_, PromptTemplateRow>(
            r#"INSERT INTO prompt_templates (name, version, content, model_hint, metadata)
               VALUES ($1, $2, $3, $4, $5)
               RETURNING id, name, version, content, model_hint, metadata, created_at, active"#,
        )
        .bind(name)
        .bind(next_version)
        .bind(content)
        .bind(&model_hint)
        .bind(&metadata)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("failed to insert prompt: {e}"))?;

        Ok(row.into())
    }

    pub async fn get_latest(&self, name: &str) -> Option<PromptTemplate> {
        sqlx::query_as::<_, PromptTemplateRow>(
            r#"SELECT id, name, version, content, model_hint, metadata, created_at, active
               FROM prompt_templates
               WHERE name = $1 AND active = TRUE
               ORDER BY version DESC
               LIMIT 1"#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(Into::into)
    }

    pub async fn get_version(&self, name: &str, version: u32) -> Option<PromptTemplate> {
        sqlx::query_as::<_, PromptTemplateRow>(
            r#"SELECT id, name, version, content, model_hint, metadata, created_at, active
               FROM prompt_templates
               WHERE name = $1 AND version = $2"#,
        )
        .bind(name)
        .bind(version as i32)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(Into::into)
    }

    pub async fn get_versions(&self, name: &str) -> Vec<PromptTemplate> {
        sqlx::query_as::<_, PromptTemplateRow>(
            r#"SELECT id, name, version, content, model_hint, metadata, created_at, active
               FROM prompt_templates
               WHERE name = $1
               ORDER BY version DESC"#,
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect()
    }

    pub async fn list(&self) -> Vec<PromptTemplate> {
        sqlx::query_as::<_, PromptTemplateRow>(
            r#"SELECT DISTINCT ON (name) id, name, version, content, model_hint, metadata, created_at, active
               FROM prompt_templates
               WHERE active = TRUE
               ORDER BY name, version DESC"#,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect()
    }

    pub async fn rollback(&self, name: &str, version: u32) -> Result<PromptTemplate, String> {
        // Deactivate all versions
        sqlx::query("UPDATE prompt_templates SET active = FALSE WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("failed to deactivate: {e}"))?;

        // Activate target version
        let row = sqlx::query_as::<_, PromptTemplateRow>(
            r#"UPDATE prompt_templates SET active = TRUE
               WHERE name = $1 AND version = $2
               RETURNING id, name, version, content, model_hint, metadata, created_at, active"#,
        )
        .bind(name)
        .bind(version as i32)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("failed to rollback: {e}"))?
        .ok_or_else(|| format!("version {version} not found for '{name}'"))?;

        Ok(row.into())
    }
}

#[cfg(feature = "postgres")]
#[derive(Debug, sqlx::FromRow)]
struct PromptTemplateRow {
    id: Uuid,
    name: String,
    version: i32,
    content: String,
    model_hint: Option<String>,
    metadata: serde_json::Value,
    created_at: chrono::DateTime<Utc>,
    active: bool,
}

#[cfg(feature = "postgres")]
impl From<PromptTemplateRow> for PromptTemplate {
    fn from(row: PromptTemplateRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            version: row.version as u32,
            content: row.content,
            model_hint: row.model_hint,
            metadata: row.metadata,
            created_at: row.created_at,
            active: row.active,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_get() {
        let store = PromptStore::new();
        store.create("greeting", "Hello {name}", None, serde_json::json!({}));
        let latest = store.get_latest("greeting").unwrap();
        assert_eq!(latest.version, 1);
        assert_eq!(latest.content, "Hello {name}");
    }

    #[test]
    fn versioning() {
        let store = PromptStore::new();
        store.create("greeting", "v1", None, serde_json::json!({}));
        store.create("greeting", "v2", None, serde_json::json!({}));
        store.create("greeting", "v3", None, serde_json::json!({}));

        let latest = store.get_latest("greeting").unwrap();
        assert_eq!(latest.version, 3);
        assert_eq!(latest.content, "v3");

        let v1 = store.get_version("greeting", 1).unwrap();
        assert_eq!(v1.content, "v1");
    }

    #[test]
    fn list_prompts() {
        let store = PromptStore::new();
        store.create("greeting", "Hello", None, serde_json::json!({}));
        store.create("farewell", "Bye", None, serde_json::json!({}));

        let list = store.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn missing_prompt_returns_none() {
        let store = PromptStore::new();
        assert!(store.get_latest("nonexistent").is_none());
    }

    #[tokio::test]
    async fn async_create_and_get_inmemory() {
        let store = PromptStore::new();
        let template = store
            .create_async("test", "content", None, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(template.version, 1);
        let latest = store.get_latest_async("test").await.unwrap();
        assert_eq!(latest.content, "content");
    }

    #[tokio::test]
    async fn async_versions_inmemory() {
        let store = PromptStore::new();
        store
            .create_async("test", "v1", None, serde_json::json!({}))
            .await
            .unwrap();
        store
            .create_async("test", "v2", None, serde_json::json!({}))
            .await
            .unwrap();
        let versions = store.get_versions_async("test").await;
        assert_eq!(versions.len(), 2);
    }
}
