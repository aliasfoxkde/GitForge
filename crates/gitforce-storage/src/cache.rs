//! Cache storage

use async_trait::async_trait;
use gitforce_common::{Error, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Cache key
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub repo_id: gitforce_common::RepoId,
    pub key: String,
    pub target: String,
}

impl CacheKey {
    pub fn new(repo_id: gitforce_common::RepoId, key: &str, target: &str) -> Self {
        Self {
            repo_id,
            key: key.to_string(),
            target: target.to_string(),
        }
    }

    /// Generate cache key hash for storage path
    pub fn hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.repo_id.to_string().as_bytes());
        hasher.update(self.key.as_bytes());
        hasher.update(self.target.as_bytes());
        hex::encode(hasher.finalize())[..16].to_string()
    }
}

/// Cache entry metadata
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub key: CacheKey,
    pub size_bytes: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub accessed_at: chrono::DateTime<chrono::Utc>,
}

/// In-memory cache store (for MVP)
pub struct InMemoryCacheStore {
    entries: Arc<RwLock<HashMap<CacheKey, (Vec<u8>, CacheEntry)>>>,
}

impl InMemoryCacheStore {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryCacheStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CacheStore for InMemoryCacheStore {
    async fn put(&self, key: CacheKey, data: Vec<u8>) -> Result<()> {
        let size_bytes = data.len() as u64;
        let now = chrono::Utc::now();

        let entry = CacheEntry {
            key: key.clone(),
            size_bytes,
            created_at: now,
            accessed_at: now,
        };

        let mut entries = self.entries.write().await;
        entries.insert(key, (data, entry));

        tracing::debug!("cached {} bytes", size_bytes);
        Ok(())
    }

    async fn get(&self, key: &CacheKey) -> Result<Option<Vec<u8>>> {
        let mut entries = self.entries.write().await;

        if let Some((data, entry)) = entries.get_mut(key) {
            // Update access time
            entry.accessed_at = chrono::Utc::now();
            tracing::debug!("cache hit for key");
            return Ok(Some(data.clone()));
        }

        tracing::debug!("cache miss for key");
        Ok(None)
    }

    async fn delete(&self, key: &CacheKey) -> Result<()> {
        let mut entries = self.entries.write().await;
        entries.remove(key);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<CacheEntry>> {
        let entries = self.entries.read().await;
        Ok(entries.values().map(|(_, e)| e.clone()).collect())
    }
}

/// Cache store trait
#[async_trait]
pub trait CacheStore: Send + Sync {
    /// Store a cache entry
    async fn put(&self, key: CacheKey, data: Vec<u8>) -> Result<()>;

    /// Retrieve a cache entry
    async fn get(&self, key: &CacheKey) -> Result<Option<Vec<u8>>>;

    /// Delete a cache entry
    async fn delete(&self, key: &CacheKey) -> Result<()>;

    /// List all cache entries
    async fn list(&self) -> Result<Vec<CacheEntry>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_store() {
        let store = InMemoryCacheStore::new();
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "cargo", "linux-x86_64");

        // Put
        store.put(key.clone(), vec![1, 2, 3]).await.unwrap();

        // Get
        let value = store.get(&key).await.unwrap();
        assert!(value.is_some());
        assert_eq!(value.unwrap(), vec![1, 2, 3]);

        // Delete
        store.delete(&key).await.unwrap();
        let value = store.get(&key).await.unwrap();
        assert!(value.is_none());
    }
}
