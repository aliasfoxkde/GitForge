//! Database connection pool management with SQLite for MVP

use gitforge_common::{Error, Result};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use std::path::Path;

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

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&connect_url)
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
                working_dir TEXT,
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
            "ALTER TABLE jobs ADD COLUMN working_dir TEXT",
            "ALTER TABLE jobs ADD COLUMN result_json TEXT",
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

    #[test]
    fn test_pool_clone() {
        // Pool is Clone, verify it can be cloned
        // We can't clone without &self but we can verify the type is Clone
        fn assert_clone<T: Clone>() {}
        assert_clone::<Pool>();
    }
}
