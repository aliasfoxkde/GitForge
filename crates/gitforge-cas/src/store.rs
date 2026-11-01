//! Content-addressed blob store with SQLite refcount index.
//!
//! Layout under `root`:
//!   `blobs/<hash[0:2]>/<hash[2:]>` — blob content
//!   `index.sqlite3`                — refcounts + last-unref timestamps
//!
//! DB write discipline: every one-shot write goes through
//! `gitforge_db::queries::persist_with_retry`; multi-statement writes open
//! with `BEGIN IMMEDIATE` (the repo's F21/F23 rule).

use std::path::PathBuf;

use gitforge_common::{Error, ErrorKind, Result};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// A content hash (hex-encoded SHA-256).
#[derive(Debug, Clone, PartialEq, Eq, Hash, sqlx::FromRow)]
pub struct BlobHash(pub String);

impl std::fmt::Display for BlobHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn hash_bytes(bytes: &[u8]) -> BlobHash {
    let digest = Sha256::digest(bytes);
    BlobHash(hex::encode(digest))
}

/// Content-addressed store: filesystem blobs + SQLite refcount index.
#[derive(Clone)]
pub struct CasStore {
    root: PathBuf,
    pool: SqlitePool,
}

/// Result of one GC sweep.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepOutcome {
    /// Blobs whose refcount is zero AND whose grace period has elapsed.
    pub deleted: u64,
    /// Bytes reclaimed.
    pub bytes_reclaimed: u64,
    /// Zero-ref blobs still inside the grace period.
    pub deferred: u64,
}

impl CasStore {
    /// Open (creating if needed) a CAS at `root`.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        tokio::fs::create_dir_all(root.join("blobs"))
            .await
            .map_err(|e| Error::with_source(ErrorKind::Storage, "CAS: create root", e))?;
        let db_path = root.join("index.sqlite3");
        use std::str::FromStr as _;
        let options = sqlx::sqlite::SqliteConnectOptions::from_str(&format!(
            "sqlite:{}?mode=rwc",
            db_path.display()
        ))
        .map_err(|e| Error::database(format!("CAS: invalid index url: {e}")))?
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(30));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect_with(options)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: open index", e))?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cas_refs (
                hash TEXT PRIMARY KEY,
                refs INTEGER NOT NULL DEFAULT 0,
                size INTEGER NOT NULL DEFAULT 0,
                last_unref_at INTEGER
            )
            "#,
        )
        .execute(&pool)
        .await
        .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: migrate index", e))?;
        Ok(Self { root, pool })
    }

    /// Close the index pool (test hygiene).
    pub async fn close(&self) {
        self.pool.close().await;
    }

    fn blob_path(&self, hash: &BlobHash) -> PathBuf {
        self.root
            .join("blobs")
            .join(&hash.0[..2])
            .join(&hash.0[2..])
    }

    /// Store bytes; idempotent. Does NOT bump any refcount — call [`Self::ref`]
    /// (or [`CasStore::put_ref`]) to take a reference.
    pub async fn put(&self, bytes: &[u8]) -> Result<BlobHash> {
        let hash = hash_bytes(bytes);
        let path = self.blob_path(&hash);
        if tokio::fs::try_exists(&path)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Storage, "CAS: stat blob", e))?
        {
            return Ok(hash);
        }
        let dir = path
            .parent()
            .ok_or_else(|| Error::storage("CAS: blob path has no parent"))?
            .to_path_buf();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Storage, "CAS: create shard dir", e))?;
        // Write to a temp name then rename: a crash mid-write must never
        // leave a partial file that a later put() would treat as valid.
        let tmp = dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            &hash.0[..8.min(hash.0.len())]
        ));
        tokio::fs::write(&tmp, bytes)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Storage, "CAS: write blob", e))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Storage, "CAS: finalize blob", e))?;
        // Record size in the index (no ref change). persist_with_retry per
        // the durable-write rule.
        let size = i64::try_from(bytes.len())
            .map_err(|_| Error::invalid_input("CAS: blob larger than i64"))?;
        let h = hash.0.clone();
        gitforge_db::queries::persist_with_retry(|| async {
            sqlx::query(
                "INSERT INTO cas_refs (hash, refs, size) VALUES (?1, 0, ?2)
                 ON CONFLICT(hash) DO NOTHING",
            )
            .bind(&h)
            .bind(size)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: index blob", e))
        })
        .await?;
        Ok(hash)
    }

    /// Fetch blob content by hash.
    pub async fn get(&self, hash: &BlobHash) -> Result<Vec<u8>> {
        let path = self.blob_path(hash);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::not_found("cas blob", hash))
            }
            Err(e) => Err(Error::with_source(ErrorKind::Storage, "CAS: read blob", e)),
        }
    }

    /// Take one reference on a blob.
    pub async fn r#ref(&self, hash: &BlobHash) -> Result<()> {
        let h = hash.0.clone();
        gitforge_db::queries::persist_with_retry(|| async {
            // The blob row must exist: referencing an unknown hash is a bug.
            let result = sqlx::query("UPDATE cas_refs SET refs = refs + 1 WHERE hash = ?1")
                .bind(&h)
                .execute(&self.pool)
                .await
                .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: ref", e))?;
            if result.rows_affected() == 0 {
                return Err(Error::not_found("cas blob", hash));
            }
            Ok(())
        })
        .await
    }

    /// Store bytes AND take a reference in one call.
    pub async fn put_ref(&self, bytes: &[u8]) -> Result<BlobHash> {
        let hash = self.put(bytes).await?;
        self.r#ref(&hash).await?;
        Ok(hash)
    }

    /// Release one reference. When the count drops to zero the blob becomes
    /// a GC candidate and `now` (unix seconds) is recorded as its last-unref
    /// time. The caller passes the clock — the decision core never reads it.
    pub async fn unref(&self, hash: &BlobHash, now_unix: i64) -> Result<()> {
        let h = hash.0.clone();
        let now_owned = now_unix;
        gitforge_db::queries::persist_with_retry(|| async {
            let result = sqlx::query(
                "UPDATE cas_refs
                 SET refs = MAX(refs - 1, 0),
                     last_unref_at = CASE WHEN refs - 1 <= 0 THEN ?2 ELSE last_unref_at END
                 WHERE hash = ?1",
            )
            .bind(&h)
            .bind(now_owned)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: unref", e))?;
            if result.rows_affected() == 0 {
                return Err(Error::not_found("cas blob", hash));
            }
            Ok(())
        })
        .await
    }

    /// Current refcount (0 if the row exists, error if not).
    pub async fn refcount(&self, hash: &BlobHash) -> Result<i64> {
        sqlx::query_scalar::<_, i64>("SELECT refs FROM cas_refs WHERE hash = ?1")
            .bind(&hash.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: refcount", e))?
            .ok_or_else(|| Error::not_found("cas blob", hash))
    }

    /// GC sweep. Deletes blobs whose refcount is zero AND whose
    /// `last_unref_at` is older than `grace_seconds` before `now_unix`.
    /// Both time inputs are injected values — no system clock reads here.
    pub async fn sweep(&self, now_unix: i64, grace_seconds: i64) -> Result<SweepOutcome> {
        // Multi-statement decision: read candidates under BEGIN IMMEDIATE so
        // a concurrent ref cannot race between select and delete.
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: sweep begin", e))?;
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT hash, size FROM cas_refs
             WHERE refs = 0
               AND last_unref_at IS NOT NULL
               AND (CAST(last_unref_at AS INTEGER) + ?1) <= ?2",
        )
        .bind(grace_seconds)
        .bind(now_unix)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: sweep select", e))?;

        let mut outcome = SweepOutcome::default();
        for (hash, size) in rows {
            let path = self.blob_path(&BlobHash(hash.clone()));
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    outcome.deleted += 1;
                    outcome.bytes_reclaimed += u64::try_from(size).unwrap_or(0);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Already gone; drop the row so it cannot linger.
                }
                Err(e) => {
                    tracing::warn!(%hash, %e, "CAS sweep: failed to delete blob; keeping row");
                    continue;
                }
            }
            sqlx::query("DELETE FROM cas_refs WHERE hash = ?1")
                .bind(&hash)
                .execute(&mut *tx)
                .await
                .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: sweep delete row", e))?;
        }

        let deferred: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM cas_refs WHERE refs = 0 AND last_unref_at IS NOT NULL",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: sweep count", e))?;
        outcome.deferred = u64::try_from(deferred).unwrap_or(0);
        tx.commit()
            .await
            .map_err(|e| Error::with_source(ErrorKind::Database, "CAS: sweep commit", e))?;
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> (CasStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CasStore::open(dir.path().join("cas"))
            .await
            .expect("open cas");
        (store, dir)
    }

    /// Injected clock: fixed unix seconds.
    fn at(unix: i64) -> i64 {
        unix
    }

    #[tokio::test]
    async fn put_get_roundtrip_is_idempotent() {
        let (store, _dir) = test_store().await;
        let h1 = store.put(b"hello world").await.expect("put");
        let h2 = store.put(b"hello world").await.expect("put again");
        assert_eq!(h1, h2);
        assert_eq!(store.get(&h1).await.expect("get"), b"hello world");
        let h3 = store.put(b"different").await.expect("put");
        assert_ne!(h1, h3);
        store.close().await;
    }

    #[tokio::test]
    async fn refcount_lifecycle() {
        let (store, _dir) = test_store().await;
        let h = store.put(b"blob").await.expect("put");
        assert_eq!(store.refcount(&h).await.expect("rc"), 0);
        store.r#ref(&h).await.expect("ref");
        store.r#ref(&h).await.expect("ref");
        assert_eq!(store.refcount(&h).await.expect("rc"), 2);
        let ts = at(1000);
        store.unref(&h, ts).await.expect("unref");
        assert_eq!(store.refcount(&h).await.expect("rc"), 1);
        store.unref(&h, ts).await.expect("unref");
        assert_eq!(store.refcount(&h).await.expect("rc"), 0);
        store.close().await;
    }

    #[tokio::test]
    async fn unref_unknown_hash_errors() {
        let (store, _dir) = test_store().await;
        let ts = at(1000);
        let err = store
            .unref(&BlobHash("deadbeef".into()), ts)
            .await
            .expect_err("must fail");
        assert_eq!(err.kind, ErrorKind::NotFound);
        store.close().await;
    }

    #[tokio::test]
    async fn sweep_respects_grace_period() {
        let (store, _dir) = test_store().await;
        let old = store.put(b"old blob").await.expect("put");
        let fresh = store.put(b"fresh blob").await.expect("put");
        let unix1000 = at(1000);
        store.unref(&old, unix1000).await.expect("unref");
        store.unref(&fresh, unix1000).await.expect("unref");

        // Sweeping at t=1000+3600 with a 24h (86400s) grace deletes nothing.
        let out = store
            .sweep(unix1000 + 3600, 86_400)
            .await
            .expect("sweep early");
        assert_eq!(out.deleted, 0);
        assert_eq!(out.deferred, 2);
        assert!(store.get(&old).await.is_ok());

        // Sweeping past the grace period deletes both.
        let out = store
            .sweep(unix1000 + 86_400 + 1, 86_400)
            .await
            .expect("sweep late");
        assert_eq!(out.deleted, 2);
        assert_eq!(out.deferred, 0);
        assert!(store.get(&old).await.is_err());
        store.close().await;
    }

    #[tokio::test]
    async fn sweep_never_deletes_live_refs() {
        let (store, _dir) = test_store().await;
        let live = store.put_ref(b"live").await.expect("put_ref");
        let dead = store.put(b"dead").await.expect("put");
        let unix = at(500);
        store.unref(&dead, unix).await.expect("unref");
        // Even far past grace, the referenced blob survives.
        let out = store.sweep(unix + 10_000_000, 86_400).await.expect("sweep");
        assert_eq!(out.deleted, 1);
        assert!(store.get(&live).await.is_ok());
        assert_eq!(store.refcount(&live).await.expect("rc"), 1);
        store.close().await;
    }

    #[tokio::test]
    async fn put_ref_takes_reference() {
        let (store, _dir) = test_store().await;
        let h = store.put_ref(b"refcounted").await.expect("put_ref");
        assert_eq!(store.refcount(&h).await.expect("rc"), 1);
        store.close().await;
    }
}
