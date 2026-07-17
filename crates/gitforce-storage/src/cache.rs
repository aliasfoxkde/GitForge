//! Cache storage

use async_trait::async_trait;
use gitforce_common::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Cache key
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key: CacheKey,
    pub size_bytes: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub accessed_at: chrono::DateTime<chrono::Utc>,
}

/// In-memory cache store (for MVP)
#[allow(clippy::type_complexity)]
#[derive(Debug)]
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

    #[test]
    fn test_cache_key_creation() {
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "cargo", "linux-x86_64");
        assert_eq!(key.key, "cargo");
        assert_eq!(key.target, "linux-x86_64");
        assert_eq!(key.repo_id, repo_id);
    }

    #[test]
    fn test_cache_key_hash() {
        let repo_id = gitforce_common::RepoId::new();
        let key1 = CacheKey::new(repo_id, "cargo", "linux-x86_64");
        let key2 = CacheKey::new(repo_id, "cargo", "linux-x86_64");
        let key3 = CacheKey::new(repo_id, "npm", "linux-x86_64");

        // Same inputs produce same hash
        assert_eq!(key1.hash(), key2.hash());
        // Different inputs produce different hash
        assert_ne!(key1.hash(), key3.hash());
        // Hash is 16 characters
        assert_eq!(key1.hash().len(), 16);
    }

    #[test]
    fn test_cache_key_hash_unique() {
        let repo_id1 = gitforce_common::RepoId::new();
        let repo_id2 = gitforce_common::RepoId::new();
        let key1 = CacheKey::new(repo_id1, "cargo", "linux-x86_64");
        let key2 = CacheKey::new(repo_id2, "cargo", "linux-x86_64");

        // Different repo_ids should produce different hashes
        assert_ne!(key1.hash(), key2.hash());
    }

    #[tokio::test]
    async fn test_cache_list() {
        let store = InMemoryCacheStore::new();
        let repo_id = gitforce_common::RepoId::new();
        let key1 = CacheKey::new(repo_id, "cargo", "linux-x86_64");
        let key2 = CacheKey::new(repo_id, "npm", "linux-x86_64");

        store.put(key1.clone(), vec![1, 2, 3]).await.unwrap();
        store.put(key2.clone(), vec![4, 5, 6]).await.unwrap();

        let entries = store.list().await.unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn test_cache_overwrite() {
        let store = InMemoryCacheStore::new();
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "cargo", "linux-x86_64");

        store.put(key.clone(), vec![1, 2, 3]).await.unwrap();
        store.put(key.clone(), vec![4, 5, 6]).await.unwrap();

        let value = store.get(&key).await.unwrap();
        assert_eq!(value.unwrap(), vec![4, 5, 6]);

        let entries = store.list().await.unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn test_cache_empty() {
        let store = InMemoryCacheStore::new();
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "cargo", "linux-x86_64");

        let value = store.get(&key).await.unwrap();
        assert!(value.is_none());

        let entries = store.list().await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_cache_entry_timestamps() {
        let store = InMemoryCacheStore::new();
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "cargo", "linux-x86_64");

        store.put(key.clone(), vec![1, 2, 3]).await.unwrap();

        let entries = store.list().await.unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.key, key);
        assert_eq!(entry.size_bytes, 3);
    }

    #[test]
    fn test_cache_key_debug() {
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "cargo", "linux-x86_64");
        let debug_str = format!("{:?}", key);
        assert!(debug_str.contains("CacheKey"));
    }

    #[test]
    fn test_cache_entry_debug() {
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "cargo", "linux-x86_64");
        let entry = CacheEntry {
            key,
            size_bytes: 100,
            created_at: chrono::Utc::now(),
            accessed_at: chrono::Utc::now(),
        };
        let debug_str = format!("{:?}", entry);
        assert!(debug_str.contains("CacheEntry"));
    }

    #[test]
    fn test_cache_key_clone() {
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "cargo", "linux-x86_64");
        let cloned = key.clone();
        assert_eq!(key, cloned);
    }

    #[test]
    fn test_in_memory_cache_store_debug() {
        let store = InMemoryCacheStore::new();
        let debug_str = format!("{:?}", store);
        assert!(debug_str.contains("InMemoryCacheStore"));
    }

    #[tokio::test]
    async fn test_cache_store_delete_nonexistent() {
        let store = InMemoryCacheStore::new();
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "nonexistent", "linux-x86_64");

        // Deleting nonexistent key should not error
        let result = store.delete(&key).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cache_multiple_keys_same_repo() {
        let store = InMemoryCacheStore::new();
        let repo_id = gitforce_common::RepoId::new();

        for i in 0..10 {
            let key = CacheKey::new(repo_id, &format!("key{}", i), "linux-x86_64");
            store.put(key, vec![i as u8]).await.unwrap();
        }

        let entries = store.list().await.unwrap();
        assert_eq!(entries.len(), 10);
    }

    #[tokio::test]
    async fn test_cache_key_with_special_characters() {
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "key-with-dashes_and_underscores", "target");
        assert_eq!(key.key, "key-with-dashes_and_underscores");
        assert_eq!(key.target, "target");
    }

    #[tokio::test]
    async fn test_cache_entry_timestamps_different() {
        let store = InMemoryCacheStore::new();
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "test", "linux");

        store.put(key.clone(), vec![1, 2, 3]).await.unwrap();

        let entries = store.list().await.unwrap();
        let entry = &entries[0];

        // created_at and accessed_at should be set
        assert!(entry.created_at <= entry.accessed_at);
    }

    #[test]
    fn test_cache_key_with_unicode() {
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "中文key", "target");
        assert_eq!(key.key, "中文key");
    }

    #[test]
    fn test_cache_key_with_empty_strings() {
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "", "");
        assert_eq!(key.key, "");
        assert_eq!(key.target, "");
    }

    #[test]
    fn test_cache_entry_size_bytes() {
        let repo_id = gitforce_common::RepoId::new();
        let key = CacheKey::new(repo_id, "test", "linux");
        let entry = CacheEntry {
            key,
            size_bytes: 1024,
            created_at: chrono::Utc::now(),
            accessed_at: chrono::Utc::now(),
        };
        assert_eq!(entry.size_bytes, 1024);
    }
}
