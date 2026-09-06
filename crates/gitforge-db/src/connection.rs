//! Database connection pool management with SQLite for MVP

use gitforge_common::{Error, Result};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

/// SQLite connection pool wrapper
#[derive(Clone)]
pub struct Pool {
    pool: SqlitePool,
}

impl Pool {
    /// Create a new connection pool from a database URL or file path
    pub async fn new(database_url: &str) -> Result<Self> {
        // Handle file paths - SQLite uses file: prefix or bare paths
        let connect_url =
            if database_url.starts_with("sqlite:") || database_url.starts_with("file:") {
                database_url.to_string()
            } else if Path::new(database_url).exists() || database_url.contains('/') {
                format!("sqlite:{}?mode=rwc", database_url)
            } else {
                // Memory database
                "sqlite::memory:".to_string()
            };

        // GitForge serves one SQLite file from several processes (gateway,
        // scheduler, git server). The default rollback journal takes an
        // exclusive lock for every write and fails concurrent writers
        // immediately with SQLITE_BUSY; under job assignment plus log
        // appends that cascaded into lost leases and dropped log chunks.
        // WAL keeps readers concurrent, and a busy timeout makes writers
        // queue instead of erroring.
        let options = SqliteConnectOptions::from_str(&connect_url)
            .map_err(|e| Error::database(format!("invalid database URL: {}", e)))?
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(|e| Error::database(format!("failed to connect to database: {}", e)))?;

        Ok(Self { pool })
    }

    /// Create an in-memory pool for testing
    pub async fn memory() -> Result<Self> {
        Self::new("sqlite::memory:").await
    }

    /// Get the underlying pool
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Run migrations to create tables
    pub async fn migrate(&self) -> Result<()> {
        // Create users table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                email TEXT NOT NULL,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'developer',
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create users table: {}", e)))?;

        // Add the role column for databases created before role persistence
        // existed. Existing accounts receive the least-privileged developer
        // role and can be promoted explicitly by an administrative workflow.
        if let Err(error) =
            sqlx::query("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'developer'")
                .execute(&self.pool)
                .await
        {
            let message = error.to_string();
            if !message.contains("duplicate column name") {
                return Err(Error::database(format!(
                    "failed to migrate users table: {}",
                    error
                )));
            }
        }

        // Create repositories table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS repositories (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                visibility TEXT NOT NULL DEFAULT 'private',
                git_path TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (owner_id) REFERENCES users(id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create repositories table: {}", e)))?;

        // Create pipelines table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS pipelines (
                id TEXT PRIMARY KEY,
                repo_id TEXT NOT NULL,
                name TEXT NOT NULL,
                trigger_type TEXT NOT NULL,
                config TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                FOREIGN KEY (repo_id) REFERENCES repositories(id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create pipelines table: {}", e)))?;

        // Create pipeline_runs table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS pipeline_runs (
                id TEXT PRIMARY KEY,
                pipeline_id TEXT NOT NULL,
                repo_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                triggered_by TEXT NOT NULL,
                commit_hash TEXT NOT NULL,
                started_at TEXT,
                finished_at TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (pipeline_id) REFERENCES pipelines(id),
                FOREIGN KEY (repo_id) REFERENCES repositories(id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create pipeline_runs table: {}", e)))?;

        // Create runners table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS runners (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                runner_type TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'offline',
                capacity INTEGER NOT NULL DEFAULT 1,
                labels TEXT NOT NULL DEFAULT '[]',
                last_heartbeat TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create runners table: {}", e)))?;

        // Create jobs table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                pipeline_run_id TEXT NOT NULL,
                name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                runner_id TEXT,
                started_at TEXT,
                finished_at TEXT,
                retry_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                commands TEXT NOT NULL DEFAULT '[]',
                image TEXT NOT NULL DEFAULT 'rust:latest',
                working_dir TEXT,
                timeout_secs INTEGER NOT NULL DEFAULT 300,
                result_json TEXT,
                FOREIGN KEY (pipeline_run_id) REFERENCES pipeline_runs(id),
                FOREIGN KEY (runner_id) REFERENCES runners(id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create jobs table: {}", e)))?;

        // Additive migration for databases created before job definitions and
        // receipts were persisted. SQLite has no portable IF NOT EXISTS form
        // for ADD COLUMN, so tolerate only the known duplicate-column case.
        for statement in [
            "ALTER TABLE jobs ADD COLUMN commands TEXT NOT NULL DEFAULT '[]'",
            "ALTER TABLE jobs ADD COLUMN image TEXT NOT NULL DEFAULT 'rust:latest'",
            "ALTER TABLE jobs ADD COLUMN working_dir TEXT",
            "ALTER TABLE jobs ADD COLUMN timeout_secs INTEGER NOT NULL DEFAULT 300",
            "ALTER TABLE jobs ADD COLUMN result_json TEXT",
            "ALTER TABLE jobs ADD COLUMN lease_token TEXT",
            "ALTER TABLE jobs ADD COLUMN lease_generation INTEGER NOT NULL DEFAULT 0",
        ] {
            if let Err(error) = sqlx::query(statement).execute(&self.pool).await {
                let message = error.to_string();
                if !message.contains("duplicate column name") {
                    return Err(Error::database(format!(
                        "failed to migrate jobs table: {}",
                        error
                    )));
                }
            }
        }

        // Create artifacts table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS artifacts (
                id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                name TEXT NOT NULL,
                path TEXT NOT NULL,
                checksum TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (job_id) REFERENCES jobs(id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create artifacts table: {}", e)))?;

        // Append-only, bounded runner log chunks. The lease fields are not
        // duplicated here: every append is authorized against the current
        // job row in JobQueries, so reassigned runners cannot append late
        // output from an old execution.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS job_log_chunks (
                job_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                chunk TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (job_id, sequence),
                FOREIGN KEY (job_id) REFERENCES jobs(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create job log chunks table: {}", e)))?;

        // Create events table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create events table: {}", e)))?;

        // Operator submissions use a durable idempotency record so retries
        // after a client timeout cannot create a second executable job.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS job_idempotency_keys (
                scope TEXT NOT NULL,
                idempotency_key TEXT NOT NULL,
                request_fingerprint TEXT NOT NULL,
                job_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (scope, idempotency_key)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create job idempotency table: {}", e)))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS publication_outbox (
                id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload TEXT NOT NULL,
                state TEXT NOT NULL CHECK (state IN ('pending', 'in_flight', 'published', 'retryable', 'permanent_failure')),
                attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
                next_attempt_at TEXT NOT NULL,
                claim_token TEXT,
                claim_until TEXT,
                external_id TEXT,
                last_error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE (job_id, provider, kind)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create publication outbox: {}", e)))?;

        // Review runs (ADR 20260905 code review contract, R3). Mirrors the
        // PostgreSQL migration in migrations/002_review_domain.sql using this
        // file's SQLite conventions: TEXT ids, RFC3339 TEXT timestamps, and
        // enforced foreign keys. The idempotency key is UNIQUE so retries can
        // create-or-get a single run per key; a matching key against a
        // different head SHA is a typed conflict handled in ReviewQueries.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS review_runs (
                id TEXT PRIMARY KEY,
                repo_id TEXT REFERENCES repositories(id) ON DELETE SET NULL,
                base_sha TEXT NOT NULL,
                head_sha TEXT NOT NULL,
                idempotency_key TEXT NOT NULL UNIQUE
                    CHECK (length(idempotency_key) > 0 AND length(idempotency_key) <= 128),
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')),
                attempt INTEGER NOT NULL DEFAULT 1,
                receipt_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create review_runs table: {}", e)))?;

        // Review findings (ADR R4/R5). Content-addressed fingerprints are
        // unique per run so retried ingestion is idempotent, and the
        // line-position invariant is enforced in the database: a `line`
        // position must carry a 1-based line; every other status must carry
        // no line.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS review_findings (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL REFERENCES review_runs(id) ON DELETE CASCADE,
                source TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                path TEXT NOT NULL,
                line INTEGER,
                severity TEXT NOT NULL,
                category TEXT NOT NULL,
                title TEXT NOT NULL,
                message TEXT NOT NULL,
                evidence TEXT,
                confidence TEXT NOT NULL,
                position_status TEXT NOT NULL
                    CHECK (position_status IN ('line', 'file', 'deleted', 'unavailable')),
                disposition TEXT NOT NULL DEFAULT 'pending',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                CHECK (
                    (position_status = 'line' AND line IS NOT NULL AND line >= 1)
                    OR (position_status <> 'line' AND line IS NULL)
                ),
                UNIQUE (run_id, fingerprint)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create review_findings table: {}", e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_review_runs_repo ON review_runs(repo_id)")
            .execute(&self.pool)
            .await
            .map_err(|e| {
                Error::database(format!("failed to create idx_review_runs_repo: {}", e))
            })?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_review_runs_status ON review_runs(status)")
            .execute(&self.pool)
            .await
            .map_err(|e| {
                Error::database(format!("failed to create idx_review_runs_status: {}", e))
            })?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_review_findings_run ON review_findings(run_id)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::database(format!("failed to create idx_review_findings_run: {}", e)))?;

        // Create indexes for performance
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status)")
            .execute(&self.pool)
            .await
            .map_err(|e| Error::database(format!("failed to create idx_jobs_status: {}", e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_jobs_pipeline_run_id ON jobs(pipeline_run_id)")
            .execute(&self.pool)
            .await
            .map_err(|e| {
                Error::database(format!("failed to create idx_jobs_pipeline_run_id: {}", e))
            })?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_pipeline_runs_pipeline_id ON pipeline_runs(pipeline_id)")
            .execute(&self.pool)
            .await
            .map_err(|e| Error::database(format!("failed to create idx_pipeline_runs_pipeline_id: {}", e)))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_repositories_owner_id ON repositories(owner_id)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| {
            Error::database(format!("failed to create idx_repositories_owner_id: {}", e))
        })?;

        tracing::info!("database migrations completed successfully");
        Ok(())
    }

    /// Check database health
    pub async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(|e| Error::database(format!("health check failed: {}", e)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_pool_creation() {
        let pool = Pool::memory().await;
        assert!(pool.is_ok());
    }

    #[tokio::test]
    async fn test_migrations() {
        let pool = Pool::memory().await.unwrap();
        let result = pool.migrate().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_health_check() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let result = pool.health_check().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_pool_new_with_memory_url() {
        // Test with explicit memory URL
        let pool = Pool::new("sqlite::memory:").await;
        assert!(pool.is_ok());
    }

    /// Concurrent writers must queue, not fail: the scheduler (lease sync),
    /// gateway, and runner log appends all write to one file. Pins WAL mode,
    /// a busy timeout, and enforced foreign keys on file-backed pools.
    #[tokio::test]
    async fn test_file_pool_enables_concurrency_pragmas() {
        let db_path =
            std::env::temp_dir().join(format!("gitforge-pragma-test-{}.db", uuid::Uuid::new_v4()));
        let pool = Pool::new(&db_path.to_string_lossy()).await.unwrap();

        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(pool.pool())
            .await
            .unwrap();
        assert_eq!(
            journal_mode, "wal",
            "file pools must use WAL for concurrent readers"
        );

        let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(pool.pool())
            .await
            .unwrap();
        assert!(
            busy_timeout > 0,
            "writers must wait on locks instead of failing"
        );

        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(pool.pool())
            .await
            .unwrap();
        assert_eq!(foreign_keys, 1, "foreign keys must be enforced");

        // Two pooled writers inserting concurrently must both succeed.
        pool.migrate().await.unwrap();
        let (left, right) = tokio::join!(
            sqlx::query("INSERT INTO events (id, event_type, payload, created_at) VALUES ('a', 't', '{}', '2026-01-01T00:00:00Z')").execute(pool.pool()),
            sqlx::query("INSERT INTO events (id, event_type, payload, created_at) VALUES ('b', 't', '{}', '2026-01-01T00:00:00Z')").execute(pool.pool()),
        );
        assert!(
            left.is_ok() && right.is_ok(),
            "concurrent writes must not fail with SQLITE_BUSY"
        );

        drop(pool);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    }

    #[test]
    fn test_pool_clone() {
        // Pool is Clone, verify it can be cloned
        // We can't clone without &self but we can verify the type is Clone
        fn assert_clone<T: Clone>() {}
        assert_clone::<Pool>();
    }
}
