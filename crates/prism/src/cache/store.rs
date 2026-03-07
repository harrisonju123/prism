use std::time::{Duration, Instant};

use dashmap::DashMap;
use sha2::{Digest, Sha256};

use crate::types::ChatCompletionRequest;
use crate::types::ChatCompletionResponse;

// ---------------------------------------------------------------------------
// ResponseCache — dispatches to InMemory, Redis, or S3 backend
// ---------------------------------------------------------------------------

pub enum ResponseCache {
    InMemory(InMemoryCache),
    #[cfg(feature = "redis-backend")]
    Redis(RedisCacheBackend),
    #[cfg(feature = "aws")]
    S3(S3CacheBackend),
}

impl ResponseCache {
    pub fn new(max_size: usize, ttl_secs: u64) -> Self {
        Self::InMemory(InMemoryCache::new(max_size, ttl_secs))
    }

    #[cfg(feature = "redis-backend")]
    pub async fn new_redis(redis_url: &str, ttl_secs: u64) -> Self {
        Self::Redis(RedisCacheBackend::new(redis_url, ttl_secs).await)
    }

    #[cfg(feature = "aws")]
    pub async fn new_s3(bucket: &str, prefix: &str, ttl_secs: u64) -> Self {
        Self::S3(S3CacheBackend::new(bucket, prefix, ttl_secs).await)
    }

    pub fn cache_key(request: &ChatCompletionRequest) -> String {
        let mut hasher = Sha256::new();
        hasher.update(request.model.as_bytes());
        for msg in &request.messages {
            hasher.update(msg.role.as_bytes());
            if let Some(content) = &msg.content {
                hasher.update(content.to_string().as_bytes());
            }
        }
        if let Some(temp) = request.temperature {
            hasher.update(temp.to_bits().to_le_bytes());
        }
        if let Some(mt) = request.max_tokens {
            hasher.update(mt.to_le_bytes());
        }
        if let Some(tools) = &request.tools {
            let tools_json = serde_json::to_string(tools).unwrap_or_default();
            hasher.update(tools_json.as_bytes());
        }
        hex::encode(hasher.finalize())
    }

    pub async fn get(&self, key: &str) -> Option<ChatCompletionResponse> {
        match self {
            Self::InMemory(inner) => inner.get(key),
            #[cfg(feature = "redis-backend")]
            Self::Redis(inner) => inner.get(key).await,
            #[cfg(feature = "aws")]
            Self::S3(inner) => inner.get(key).await,
        }
    }

    pub async fn insert(&self, key: String, response: ChatCompletionResponse) {
        match self {
            Self::InMemory(inner) => inner.insert(key, response),
            #[cfg(feature = "redis-backend")]
            Self::Redis(inner) => inner.insert(key, response).await,
            #[cfg(feature = "aws")]
            Self::S3(inner) => inner.insert(key, response).await,
        }
    }

    pub fn prune_expired(&self) {
        match self {
            Self::InMemory(inner) => inner.prune_expired(),
            #[cfg(feature = "redis-backend")]
            Self::Redis(_) => {} // Redis handles TTL
            #[cfg(feature = "aws")]
            Self::S3(_) => {}    // S3 doesn't need pruning
        }
    }
}

// ---------------------------------------------------------------------------
// In-memory backend (original implementation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CacheEntry {
    response: ChatCompletionResponse,
    inserted_at: Instant,
    last_accessed: Instant,
}

pub struct InMemoryCache {
    entries: DashMap<String, CacheEntry>,
    max_size: usize,
    ttl: Duration,
}

impl InMemoryCache {
    pub fn new(max_size: usize, ttl_secs: u64) -> Self {
        Self {
            entries: DashMap::new(),
            max_size,
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    pub fn get(&self, key: &str) -> Option<ChatCompletionResponse> {
        let now = Instant::now();
        let mut entry = self.entries.get_mut(key)?;
        if now.duration_since(entry.inserted_at) > self.ttl {
            drop(entry);
            self.entries.remove(key);
            return None;
        }
        entry.last_accessed = now;
        Some(entry.response.clone())
    }

    pub fn insert(&self, key: String, response: ChatCompletionResponse) {
        if self.entries.len() >= self.max_size {
            self.evict_lru();
        }
        let now = Instant::now();
        self.entries.insert(
            key,
            CacheEntry {
                response,
                inserted_at: now,
                last_accessed: now,
            },
        );
    }

    pub fn prune_expired(&self) {
        let now = Instant::now();
        let ttl = self.ttl;
        self.entries
            .retain(|_, entry| now.duration_since(entry.inserted_at) <= ttl);
    }

    fn evict_lru(&self) {
        let mut oldest_key: Option<String> = None;
        let mut oldest_time = Instant::now();
        for entry in self.entries.iter() {
            if entry.value().last_accessed < oldest_time {
                oldest_time = entry.value().last_accessed;
                oldest_key = Some(entry.key().clone());
            }
        }
        if let Some(key) = oldest_key {
            self.entries.remove(&key);
        }
    }
}

// ---------------------------------------------------------------------------
// Redis cache backend
// ---------------------------------------------------------------------------

#[cfg(feature = "redis-backend")]
pub struct RedisCacheBackend {
    conn: Option<redis::aio::MultiplexedConnection>,
    ttl_secs: u64,
    fallback: InMemoryCache,
}

#[cfg(feature = "redis-backend")]
impl RedisCacheBackend {
    pub async fn new(redis_url: &str, ttl_secs: u64) -> Self {
        let conn = match redis::Client::open(redis_url) {
            Ok(client) => match client.get_multiplexed_async_connection().await {
                Ok(conn) => {
                    tracing::info!(%redis_url, ttl_secs, "redis cache backend connected");
                    Some(conn)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "redis cache connection failed, using fallback");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "redis cache client error, using fallback");
                None
            }
        };
        Self {
            conn,
            ttl_secs,
            fallback: InMemoryCache::new(1000, ttl_secs),
        }
    }

    pub async fn get(&self, key: &str) -> Option<ChatCompletionResponse> {
        if let Some(ref conn) = self.conn {
            let mut conn = conn.clone();
            let redis_key = format!("cache:{key}");
            match redis::cmd("GET")
                .arg(&redis_key)
                .query_async::<Option<String>>(&mut conn)
                .await
            {
                Ok(Some(json)) => match serde_json::from_str::<ChatCompletionResponse>(&json) {
                    Ok(resp) => return Some(resp),
                    Err(e) => {
                        tracing::warn!(error = %e, "redis cache deserialization failed");
                    }
                },
                Ok(None) => return None,
                Err(e) => {
                    tracing::warn!(error = %e, "redis cache GET failed, trying fallback");
                }
            }
        }
        self.fallback.get(key)
    }

    pub async fn insert(&self, key: String, response: ChatCompletionResponse) {
        if let Some(ref conn) = self.conn {
            let mut conn = conn.clone();
            let redis_key = format!("cache:{key}");
            match serde_json::to_string(&response) {
                Ok(json) => {
                    match redis::cmd("SET")
                        .arg(&redis_key)
                        .arg(&json)
                        .arg("EX")
                        .arg(self.ttl_secs)
                        .query_async::<()>(&mut conn)
                        .await
                    {
                        Ok(()) => return,
                        Err(e) => {
                            tracing::warn!(error = %e, "redis cache SET failed, using fallback");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "redis cache serialization failed");
                }
            }
        }
        self.fallback.insert(key, response);
    }
}

// ---------------------------------------------------------------------------
// S3 cache backend
// ---------------------------------------------------------------------------

#[cfg(feature = "aws")]
pub struct S3CacheBackend {
    client: Option<aws_sdk_s3::Client>,
    bucket: String,
    prefix: String,
    ttl_secs: u64,
    fallback: InMemoryCache,
}

#[cfg(feature = "aws")]
impl S3CacheBackend {
    pub async fn new(bucket: &str, prefix: &str, ttl_secs: u64) -> Self {
        let client = match tokio::time::timeout(
            Duration::from_secs(5),
            aws_config::load_defaults(aws_config::BehaviorVersion::latest()),
        )
        .await
        {
            Ok(config) => {
                tracing::info!(%bucket, %prefix, ttl_secs, "s3 cache backend initialized");
                Some(aws_sdk_s3::Client::new(&config))
            }
            Err(_) => {
                tracing::warn!("s3 config load timeout, using fallback");
                None
            }
        };
        Self {
            client,
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
            ttl_secs,
            fallback: InMemoryCache::new(1000, ttl_secs),
        }
    }

    fn s3_key(&self, key: &str) -> String {
        format!("{}{}.json", self.prefix, key)
    }

    pub async fn get(&self, key: &str) -> Option<ChatCompletionResponse> {
        if let Some(ref client) = self.client {
            let s3_key = self.s3_key(key);
            match client
                .get_object()
                .bucket(&self.bucket)
                .key(&s3_key)
                .send()
                .await
            {
                Ok(output) => match output.body.collect().await {
                    Ok(bytes) => {
                        let data = bytes.into_bytes();
                        match serde_json::from_slice::<ChatCompletionResponse>(&data) {
                            Ok(resp) => return Some(resp),
                            Err(e) => {
                                tracing::warn!(error = %e, "s3 cache deserialization failed");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "s3 cache body read failed");
                    }
                },
                Err(e) => {
                    // NoSuchKey is expected for cache misses
                    let is_not_found = e.as_service_error().is_some_and(|se| se.is_no_such_key());
                    if !is_not_found {
                        tracing::warn!(error = %e, "s3 cache GET failed, trying fallback");
                    }
                    return self.fallback.get(key);
                }
            }
        }
        self.fallback.get(key)
    }

    pub async fn insert(&self, key: String, response: ChatCompletionResponse) {
        if let Some(ref client) = self.client {
            let s3_key = self.s3_key(&key);
            match serde_json::to_vec(&response) {
                Ok(json_bytes) => {
                    match client
                        .put_object()
                        .bucket(&self.bucket)
                        .key(&s3_key)
                        .body(json_bytes.into())
                        .content_type("application/json")
                        .send()
                        .await
                    {
                        Ok(_) => return,
                        Err(e) => {
                            tracing::warn!(error = %e, "s3 cache PUT failed, using fallback");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "s3 cache serialization failed");
                }
            }
        }
        self.fallback.insert(key, response);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatCompletionRequest, ChatCompletionResponse, Choice, Message, Usage};

    fn make_request(model: &str, user_msg: &str) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: model.to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: Some(serde_json::Value::String(user_msg.to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: false,
            stream_options: None,
            stop: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            user: None,
            extra: serde_json::Map::new(),
        }
    }

    fn make_response(text: &str) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "test-id".to_string(),
            object: "chat.completion".to_string(),
            created: 1234567890,
            model: "test-model".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: "assistant".to_string(),
                    content: Some(serde_json::Value::String(text.to_string())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: serde_json::Map::new(),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
                ..Default::default()
            }),
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn key_deterministic() {
        let req = make_request("gpt-4o", "hello");
        let k1 = ResponseCache::cache_key(&req);
        let k2 = ResponseCache::cache_key(&req);
        assert_eq!(k1, k2);
    }

    #[test]
    fn key_varies_on_model() {
        let r1 = make_request("gpt-4o", "hello");
        let r2 = make_request("sonnet", "hello");
        assert_ne!(ResponseCache::cache_key(&r1), ResponseCache::cache_key(&r2));
    }

    #[test]
    fn key_varies_on_temperature() {
        let mut r1 = make_request("gpt-4o", "hello");
        let mut r2 = make_request("gpt-4o", "hello");
        r1.temperature = Some(0.5);
        r2.temperature = Some(0.9);
        assert_ne!(ResponseCache::cache_key(&r1), ResponseCache::cache_key(&r2));
    }

    #[test]
    fn key_varies_on_messages() {
        let r1 = make_request("gpt-4o", "hello");
        let r2 = make_request("gpt-4o", "goodbye");
        assert_ne!(ResponseCache::cache_key(&r1), ResponseCache::cache_key(&r2));
    }

    #[test]
    fn key_varies_on_tools() {
        let mut r1 = make_request("gpt-4o", "hello");
        let mut r2 = make_request("gpt-4o", "hello");
        r1.tools = Some(vec![crate::types::Tool {
            r#type: "function".to_string(),
            function: crate::types::ToolFunction {
                name: "get_weather".to_string(),
                description: None,
                parameters: None,
            },
        }]);
        r2.tools = None;
        assert_ne!(ResponseCache::cache_key(&r1), ResponseCache::cache_key(&r2));
    }

    #[tokio::test]
    async fn miss_on_empty_cache() {
        let cache = ResponseCache::new(10, 3600);
        assert!(cache.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn hit_after_insert() {
        let cache = ResponseCache::new(10, 3600);
        let resp = make_response("hello world");
        cache.insert("key1".to_string(), resp.clone()).await;
        let cached = cache.get("key1").await.unwrap();
        assert_eq!(cached.id, resp.id);
    }

    #[tokio::test]
    async fn ttl_expiry() {
        let cache = ResponseCache::new(10, 0);
        let resp = make_response("hello");
        cache.insert("key1".to_string(), resp).await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(cache.get("key1").await.is_none());
    }

    #[tokio::test]
    async fn eviction_at_capacity() {
        let cache = ResponseCache::new(2, 3600);
        cache.insert("a".to_string(), make_response("a")).await;
        cache.insert("b".to_string(), make_response("b")).await;
        let _ = cache.get("b").await;
        cache.insert("c".to_string(), make_response("c")).await;
        assert!(cache.get("a").await.is_none());
        assert!(cache.get("b").await.is_some());
        assert!(cache.get("c").await.is_some());
    }

    #[tokio::test]
    async fn prune_removes_expired() {
        let cache = ResponseCache::new(10, 0);
        cache.insert("a".to_string(), make_response("a")).await;
        cache.insert("b".to_string(), make_response("b")).await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        cache.prune_expired();
    }

    #[cfg(feature = "redis-backend")]
    #[tokio::test]
    async fn redis_backend_fallback() {
        let cache = ResponseCache::new_redis("redis://localhost:1", 3600).await;
        cache
            .insert("key1".to_string(), make_response("test"))
            .await;
        assert!(cache.get("key1").await.is_some());
    }

    #[cfg(feature = "aws")]
    #[tokio::test]
    async fn s3_backend_fallback() {
        let cache = ResponseCache::new_s3("test-bucket", "cache/", 3600).await;
        cache
            .insert("key1".to_string(), make_response("test"))
            .await;
        assert!(cache.get("key1").await.is_some());
    }

    #[test]
    fn serialization_roundtrip() {
        let resp = make_response("hello world");
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: ChatCompletionResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, resp.id);
        assert_eq!(deserialized.model, resp.model);
        assert_eq!(deserialized.choices.len(), resp.choices.len());
    }

    #[cfg(feature = "aws")]
    #[test]
    fn s3_key_format() {
        let backend = S3CacheBackend {
            client: None,
            bucket: "test".to_string(),
            prefix: "cache/".to_string(),
            ttl_secs: 3600,
            fallback: InMemoryCache::new(10, 3600),
        };
        let key = backend.s3_key("abc123");
        assert_eq!(key, "cache/abc123.json");
    }
}
