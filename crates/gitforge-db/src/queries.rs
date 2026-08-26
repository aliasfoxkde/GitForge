//! Database queries implementation using SQLite
//!
//! This module provides real SQLite query implementations for all database operations.

use crate::models::JobStatus;
use crate::Pool;
use chrono::{DateTime, Utc};
use gitforge_common::{Error, JobId, PipelineId, PipelineRunId, RepoId, Result, RunnerId, UserId};
use sqlx::Row;
use uuid::Uuid;

// ============================================================================
// Repository Queries
// ============================================================================

pub struct RepoQueries;

impl RepoQueries {
    /// Create a new repository
    pub async fn create(pool: &Pool, repo: &crate::models::Repository) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO repositories (id, name, owner_id, visibility, git_path, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(repo.id.to_string())
        .bind(&repo.name)
        .bind(repo.owner_id.to_string())
        .bind(&repo.visibility)
        .bind(&repo.git_path)
        .bind(repo.created_at.to_rfc3339())
        .bind(repo.updated_at.to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create repository: {}", e)))?;
        Ok(())
    }

    /// Get a repository by ID
    pub async fn get(pool: &Pool, id: RepoId) -> Result<Option<crate::models::Repository>> {
        let row = sqlx::query("SELECT * FROM repositories WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get repository: {}", e)))?;

        match row {
            Some(row) => Ok(Some(crate::models::Repository {
                id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                name: row.get("name"),
                owner_id: UserId::from(Uuid::parse_str(&row.get::<String, _>("owner_id")).unwrap()),
                visibility: row.get("visibility"),
                git_path: row.get("git_path"),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
                updated_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("updated_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })),
            None => Ok(None),
        }
    }

    /// List repositories by owner
    pub async fn list_by_owner(
        pool: &Pool,
        owner_id: UserId,
    ) -> Result<Vec<crate::models::Repository>> {
        let rows = sqlx::query("SELECT * FROM repositories WHERE owner_id = ?")
            .bind(owner_id.to_string())
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list repositories: {}", e)))?;

        let repos = rows
            .into_iter()
            .map(|row| crate::models::Repository {
                id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                name: row.get("name"),
                owner_id: UserId::from(Uuid::parse_str(&row.get::<String, _>("owner_id")).unwrap()),
                visibility: row.get("visibility"),
                git_path: row.get("git_path"),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
                updated_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("updated_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(repos)
    }

    /// Delete a repository
    pub async fn delete(pool: &Pool, id: RepoId) -> Result<()> {
        sqlx::query("DELETE FROM repositories WHERE id = ?")
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to delete repository: {}", e)))?;
        Ok(())
    }

    /// List all repositories
    pub async fn list(pool: &Pool) -> Result<Vec<crate::models::Repository>> {
        let rows = sqlx::query("SELECT * FROM repositories ORDER BY created_at DESC")
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list repositories: {}", e)))?;

        let repos = rows
            .into_iter()
            .map(|row| crate::models::Repository {
                id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                name: row.get("name"),
                owner_id: UserId::from(Uuid::parse_str(&row.get::<String, _>("owner_id")).unwrap()),
                visibility: row.get("visibility"),
                git_path: row.get("git_path"),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
                updated_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("updated_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(repos)
    }

    /// Get a repository by owner username and repository name
    pub async fn get_by_owner_and_name(
        pool: &Pool,
        owner_username: &str,
        repo_name: &str,
    ) -> Result<Option<crate::models::Repository>> {
        let row = sqlx::query(
            r#"
            SELECT r.* FROM repositories r
            JOIN users u ON r.owner_id = u.id
            WHERE u.username = ? AND r.name = ?
            "#,
        )
        .bind(owner_username)
        .bind(repo_name)
        .fetch_optional(pool.pool())
        .await
        .map_err(|e| {
            Error::database(format!("failed to get repository by owner and name: {}", e))
        })?;

        match row {
            Some(row) => Ok(Some(crate::models::Repository {
                id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                name: row.get("name"),
                owner_id: UserId::from(Uuid::parse_str(&row.get::<String, _>("owner_id")).unwrap()),
                visibility: row.get("visibility"),
                git_path: row.get("git_path"),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
                updated_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("updated_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })),
            None => Ok(None),
        }
    }
}

// ============================================================================
// User Queries
// ============================================================================

pub struct UserQueries;

impl UserQueries {
    /// Get the persisted role for a user. This is kept separate from the
    /// legacy User model so existing callers remain source-compatible while
    /// the schema gains least-privilege role persistence.
    pub async fn get_role(pool: &Pool, id: UserId) -> Result<Option<String>> {
        let row = sqlx::query("SELECT role FROM users WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get user role: {}", e)))?;
        Ok(row.map(|row| row.get::<String, _>("role")))
    }

    /// Set a user's persisted least-privilege role.
    pub async fn set_role(pool: &Pool, id: UserId, role: &str) -> Result<bool> {
        let result = sqlx::query("UPDATE users SET role = ? WHERE id = ?")
            .bind(role)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to set user role: {}", e)))?;
        Ok(result.rows_affected() == 1)
    }

    /// Count users currently holding a persisted role.
    pub async fn count_role(pool: &Pool, role: &str) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM users WHERE role = ?")
            .bind(role)
            .fetch_one(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to count user roles: {}", e)))?;
        Ok(row.get::<i64, _>("count"))
    }

    /// Create a new user
    pub async fn create(pool: &Pool, user: &crate::models::User) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO users (id, username, email, password_hash, created_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(user.id.to_string())
        .bind(&user.username)
        .bind(&user.email)
        .bind(&user.password_hash)
        .bind(user.created_at.to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create user: {}", e)))?;
        Ok(())
    }

    /// Get a user by ID
    pub async fn get(pool: &Pool, id: UserId) -> Result<Option<crate::models::User>> {
        let row = sqlx::query("SELECT * FROM users WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get user: {}", e)))?;

        match row {
            Some(row) => Ok(Some(crate::models::User {
                id: UserId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                username: row.get("username"),
                email: row.get("email"),
                password_hash: row.get("password_hash"),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })),
            None => Ok(None),
        }
    }

    /// Get a user by username
    pub async fn get_by_username(
        pool: &Pool,
        username: &str,
    ) -> Result<Option<crate::models::User>> {
        let row = sqlx::query("SELECT * FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get user by username: {}", e)))?;

        match row {
            Some(row) => Ok(Some(crate::models::User {
                id: UserId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                username: row.get("username"),
                email: row.get("email"),
                password_hash: row.get("password_hash"),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })),
            None => Ok(None),
        }
    }

    /// List all users
    pub async fn list(pool: &Pool) -> Result<Vec<crate::models::User>> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY created_at DESC")
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list users: {}", e)))?;

        let users = rows
            .into_iter()
            .map(|row| crate::models::User {
                id: UserId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                username: row.get("username"),
                email: row.get("email"),
                password_hash: row.get("password_hash"),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(users)
    }
}

// ============================================================================
// Pipeline Queries
// ============================================================================

pub struct PipelineQueries;

impl PipelineQueries {
    /// Create a new pipeline
    pub async fn create(pool: &Pool, pipeline: &crate::models::Pipeline) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO pipelines (id, repo_id, name, trigger_type, config, created_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(pipeline.id.to_string())
        .bind(pipeline.repo_id.to_string())
        .bind(&pipeline.name)
        .bind(&pipeline.trigger_type)
        .bind(pipeline.config.to_string())
        .bind(pipeline.created_at.to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create pipeline: {}", e)))?;
        Ok(())
    }

    /// Get a pipeline by ID
    pub async fn get(pool: &Pool, id: PipelineId) -> Result<Option<crate::models::Pipeline>> {
        let row = sqlx::query("SELECT * FROM pipelines WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get pipeline: {}", e)))?;

        match row {
            Some(row) => Ok(Some(crate::models::Pipeline {
                id: PipelineId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                repo_id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("repo_id")).unwrap()),
                name: row.get("name"),
                trigger_type: row.get("trigger_type"),
                config: serde_json::from_str(&row.get::<String, _>("config")).unwrap_or_default(),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })),
            None => Ok(None),
        }
    }

    /// List pipelines by repository
    pub async fn list_by_repo(
        pool: &Pool,
        repo_id: RepoId,
    ) -> Result<Vec<crate::models::Pipeline>> {
        let rows =
            sqlx::query("SELECT * FROM pipelines WHERE repo_id = ? ORDER BY created_at DESC")
                .bind(repo_id.to_string())
                .fetch_all(pool.pool())
                .await
                .map_err(|e| Error::database(format!("failed to list pipelines: {}", e)))?;

        let pipelines = rows
            .into_iter()
            .map(|row| crate::models::Pipeline {
                id: PipelineId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                repo_id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("repo_id")).unwrap()),
                name: row.get("name"),
                trigger_type: row.get("trigger_type"),
                config: serde_json::from_str(&row.get::<String, _>("config")).unwrap_or_default(),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(pipelines)
    }

    /// List all pipelines
    pub async fn list(pool: &Pool) -> Result<Vec<crate::models::Pipeline>> {
        let rows = sqlx::query("SELECT * FROM pipelines ORDER BY created_at DESC")
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list pipelines: {}", e)))?;

        let pipelines = rows
            .into_iter()
            .map(|row| crate::models::Pipeline {
                id: PipelineId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                repo_id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("repo_id")).unwrap()),
                name: row.get("name"),
                trigger_type: row.get("trigger_type"),
                config: serde_json::from_str(&row.get::<String, _>("config")).unwrap_or_default(),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(pipelines)
    }
}

// ============================================================================
// Pipeline Run Queries
// ============================================================================

pub struct PipelineRunQueries;

impl PipelineRunQueries {
    /// Create a new pipeline run
    pub async fn create(pool: &Pool, run: &crate::models::PipelineRun) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO pipeline_runs (id, pipeline_id, repo_id, status, triggered_by, commit_hash, started_at, finished_at, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(run.id.to_string())
        .bind(run.pipeline_id.to_string())
        .bind(run.repo_id.to_string())
        .bind(&run.status)
        .bind(&run.triggered_by)
        .bind(&run.commit_hash)
        .bind(run.started_at.map(|dt| dt.to_rfc3339()))
        .bind(run.finished_at.map(|dt| dt.to_rfc3339()))
        .bind(run.created_at.to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create pipeline run: {}", e)))?;
        Ok(())
    }

    /// Get a pipeline run by ID
    pub async fn get(pool: &Pool, id: PipelineRunId) -> Result<Option<crate::models::PipelineRun>> {
        let row = sqlx::query("SELECT * FROM pipeline_runs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get pipeline run: {}", e)))?;

        match row {
            Some(row) => Ok(Some(crate::models::PipelineRun {
                id: PipelineRunId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                pipeline_id: PipelineId::from(
                    Uuid::parse_str(&row.get::<String, _>("pipeline_id")).unwrap(),
                ),
                repo_id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("repo_id")).unwrap()),
                status: row.get("status"),
                triggered_by: row.get("triggered_by"),
                commit_hash: row.get("commit_hash"),
                started_at: row
                    .get::<Option<String>, _>("started_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                finished_at: row
                    .get::<Option<String>, _>("finished_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })),
            None => Ok(None),
        }
    }

    /// Update pipeline run status
    pub async fn update_status(pool: &Pool, id: PipelineRunId, status: &str) -> Result<()> {
        let finished_at = matches!(
            status,
            "succeeded" | "failed" | "cancelled" | "timed_out" | "timeout" | "timed-out"
        )
        .then(|| Utc::now().to_rfc3339());
        sqlx::query(
            "UPDATE pipeline_runs SET status = ?, finished_at = COALESCE(?, finished_at) WHERE id = ?",
        )
            .bind(status)
            .bind(finished_at)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to update pipeline run status: {}", e)))?;
        Ok(())
    }

    /// List pipeline runs by pipeline
    pub async fn list_by_pipeline(
        pool: &Pool,
        pipeline_id: PipelineId,
    ) -> Result<Vec<crate::models::PipelineRun>> {
        let rows = sqlx::query(
            "SELECT * FROM pipeline_runs WHERE pipeline_id = ? ORDER BY created_at DESC",
        )
        .bind(pipeline_id.to_string())
        .fetch_all(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to list pipeline runs: {}", e)))?;

        let runs = rows
            .into_iter()
            .map(|row| crate::models::PipelineRun {
                id: PipelineRunId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                pipeline_id: PipelineId::from(
                    Uuid::parse_str(&row.get::<String, _>("pipeline_id")).unwrap(),
                ),
                repo_id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("repo_id")).unwrap()),
                status: row.get("status"),
                triggered_by: row.get("triggered_by"),
                commit_hash: row.get("commit_hash"),
                started_at: row
                    .get::<Option<String>, _>("started_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                finished_at: row
                    .get::<Option<String>, _>("finished_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(runs)
    }

    /// List all pipeline runs
    pub async fn list(pool: &Pool) -> Result<Vec<crate::models::PipelineRun>> {
        let rows = sqlx::query("SELECT * FROM pipeline_runs ORDER BY created_at DESC")
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list pipeline runs: {}", e)))?;

        let runs = rows
            .into_iter()
            .map(|row| crate::models::PipelineRun {
                id: PipelineRunId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                pipeline_id: PipelineId::from(
                    Uuid::parse_str(&row.get::<String, _>("pipeline_id")).unwrap(),
                ),
                repo_id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("repo_id")).unwrap()),
                status: row.get("status"),
                triggered_by: row.get("triggered_by"),
                commit_hash: row.get("commit_hash"),
                started_at: row
                    .get::<Option<String>, _>("started_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                finished_at: row
                    .get::<Option<String>, _>("finished_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(runs)
    }
}

// ============================================================================
// Job Queries
// ============================================================================

pub struct JobQueries;

impl JobQueries {
    /// Return the existing submission record for a scoped idempotency key.
    pub async fn get_idempotency(
        pool: &Pool,
        scope: &str,
        idempotency_key: &str,
    ) -> Result<Option<(JobId, String)>> {
        let row = sqlx::query(
            "SELECT job_id, request_fingerprint FROM job_idempotency_keys WHERE scope = ? AND idempotency_key = ?",
        )
        .bind(scope)
        .bind(idempotency_key)
        .fetch_optional(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to get job idempotency key: {}", e)))?;
        row.map(|row| {
            let job_id = Uuid::parse_str(&row.get::<String, _>("job_id"))
                .map(JobId::from)
                .map_err(|e| Error::database(format!("invalid stored idempotent job ID: {}", e)))?;
            Ok((job_id, row.get("request_fingerprint")))
        })
        .transpose()
    }

    /// Reserve an idempotency key. SQLite's conflict result makes this safe
    /// when multiple control-plane retries arrive concurrently.
    pub async fn reserve_idempotency(
        pool: &Pool,
        scope: &str,
        idempotency_key: &str,
        request_fingerprint: &str,
        job_id: JobId,
    ) -> Result<bool> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO job_idempotency_keys (scope, idempotency_key, request_fingerprint, job_id, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(scope)
        .bind(idempotency_key)
        .bind(request_fingerprint)
        .bind(job_id.to_string())
        .bind(Utc::now().to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to reserve job idempotency key: {}", e)))?;
        Ok(result.rows_affected() == 1)
    }

    /// Release a reservation when the first job-row write fails before the
    /// submission has become executable. This avoids poisoning a client key
    /// after a transient database failure.
    pub async fn delete_idempotency(pool: &Pool, scope: &str, idempotency_key: &str) -> Result<()> {
        sqlx::query("DELETE FROM job_idempotency_keys WHERE scope = ? AND idempotency_key = ?")
            .bind(scope)
            .bind(idempotency_key)
            .execute(pool.pool())
            .await
            .map_err(|e| {
                Error::database(format!("failed to release job idempotency key: {}", e))
            })?;
        Ok(())
    }

    /// Create a new job
    pub async fn create(pool: &Pool, job: &crate::models::Job) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO jobs (id, pipeline_run_id, name, status, runner_id, started_at, finished_at, retry_count, created_at, commands, working_dir, result_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(job.id.to_string())
        .bind(job.pipeline_run_id.to_string())
        .bind(&job.name)
        .bind(&job.status)
        .bind(job.runner_id.map(|id| id.to_string()))
        .bind(job.started_at.map(|dt| dt.to_rfc3339()))
        .bind(job.finished_at.map(|dt| dt.to_rfc3339()))
        .bind(job.retry_count)
        .bind(job.created_at.to_rfc3339())
        .bind(serde_json::to_string(&job.commands).unwrap_or_else(|_| "[]".to_string()))
        .bind(&job.working_dir)
        .bind(&job.result_json)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create job: {}", e)))?;
        Ok(())
    }

    /// Get a job by ID
    pub async fn get(pool: &Pool, id: JobId) -> Result<Option<crate::models::Job>> {
        let row = sqlx::query("SELECT * FROM jobs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get job: {}", e)))?;

        match row {
            Some(row) => Ok(Some(crate::models::Job {
                id: JobId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                pipeline_run_id: PipelineRunId::from(
                    Uuid::parse_str(&row.get::<String, _>("pipeline_run_id")).unwrap(),
                ),
                name: row.get("name"),
                status: row.get("status"),
                runner_id: row
                    .get::<Option<String>, _>("runner_id")
                    .and_then(|s| Uuid::parse_str(&s).ok().map(RunnerId::from)),
                started_at: row
                    .get::<Option<String>, _>("started_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                finished_at: row
                    .get::<Option<String>, _>("finished_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                retry_count: row.get("retry_count"),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
                commands: serde_json::from_str(
                    &row.get::<Option<String>, _>("commands")
                        .unwrap_or_else(|| "[]".to_string()),
                )
                .unwrap_or_default(),
                working_dir: row.get("working_dir"),
                result_json: row.get("result_json"),
            })),
            None => Ok(None),
        }
    }

    /// Update job status
    pub async fn update_status(pool: &Pool, id: JobId, status: &str) -> Result<()> {
        sqlx::query("UPDATE jobs SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to update job status: {}", e)))?;
        Ok(())
    }

    /// Requeue an assigned job and clear its runner fencing token.
    pub async fn requeue(pool: &Pool, id: JobId) -> Result<()> {
        sqlx::query(
            "UPDATE jobs SET status = 'queued', runner_id = NULL, started_at = NULL, lease_token = NULL WHERE id = ? AND status = 'assigned'",
        )
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to requeue job: {}", e)))?;
        Ok(())
    }

    /// Persist the executable definition for a job. This is intentionally
    /// separate from status transitions so queueing remains idempotent.
    pub async fn set_definition(
        pool: &Pool,
        id: JobId,
        commands: &[String],
        working_dir: Option<&str>,
    ) -> Result<()> {
        let commands_json = serde_json::to_string(commands)
            .map_err(|e| Error::database(format!("failed to encode job commands: {}", e)))?;
        sqlx::query("UPDATE jobs SET commands = ?, working_dir = ? WHERE id = ?")
            .bind(commands_json)
            .bind(working_dir)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to persist job definition: {}", e)))?;
        Ok(())
    }

    /// Persist a terminal execution receipt. A repeated identical completion
    /// is idempotent; a conflicting completion is rejected.
    pub async fn complete(pool: &Pool, id: JobId, status: &str, result_json: &str) -> Result<()> {
        let existing = Self::get(pool, id).await?;
        if let Some(job) = existing {
            if let Some(current) = JobStatus::from_str(&job.status) {
                if current.is_terminal() {
                    if job.result_json.as_deref() == Some(result_json) {
                        return Ok(());
                    }
                    return Err(Error::invalid_input(
                        "job already has a different terminal receipt",
                    ));
                }
            }
        } else {
            return Err(Error::not_found("job", id));
        }

        sqlx::query("UPDATE jobs SET status = ?, finished_at = ?, result_json = ? WHERE id = ?")
            .bind(status)
            .bind(Utc::now().to_rfc3339())
            .bind(result_json)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to persist job receipt: {}", e)))?;
        Ok(())
    }

    /// Assign a runner to a job
    pub async fn assign(pool: &Pool, id: JobId, runner_id: RunnerId) -> Result<()> {
        sqlx::query("UPDATE jobs SET runner_id = ?, status = 'assigned' WHERE id = ?")
            .bind(runner_id.to_string())
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to assign job: {}", e)))?;
        Ok(())
    }

    /// Atomically assign a queued job and advance its durable fencing
    /// generation. A false result means another scheduler won the race.
    pub async fn assign_with_lease(
        pool: &Pool,
        id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE jobs SET runner_id = ?, status = 'assigned', lease_token = ?, lease_generation = lease_generation + 1 WHERE id = ? AND status IN ('pending', 'queued') AND runner_id IS NULL",
        )
        .bind(runner_id.to_string())
        .bind(lease_token)
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to assign job lease: {}", e)))?;
        Ok(result.rows_affected() == 1)
    }

    /// Persist the assigned-to-running lifecycle transition.
    pub async fn start(pool: &Pool, id: JobId) -> Result<()> {
        sqlx::query(
            "UPDATE jobs SET status = 'running', started_at = COALESCE(started_at, ?) WHERE id = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to start job: {}", e)))?;
        Ok(())
    }

    /// Start a job only when the durable runner lease still matches.
    pub async fn start_with_lease(
        pool: &Pool,
        id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE jobs SET status = 'running', started_at = COALESCE(started_at, ?) WHERE id = ? AND runner_id = ? AND lease_token = ? AND status = 'assigned'",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .bind(runner_id.to_string())
        .bind(lease_token)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to start job with lease: {}", e)))?;
        Ok(result.rows_affected() == 1)
    }

    /// Complete a job only when the durable runner lease still matches. The
    /// lease is cleared as part of the same conditional update, fencing late
    /// completion messages after reassignment or terminal transition.
    pub async fn complete_with_lease(
        pool: &Pool,
        id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
        status: &str,
        result_json: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE jobs SET status = ?, finished_at = ?, result_json = ?, lease_token = NULL WHERE id = ? AND runner_id = ? AND lease_token = ? AND status IN ('assigned', 'running')",
        )
        .bind(status)
        .bind(Utc::now().to_rfc3339())
        .bind(result_json)
        .bind(id.to_string())
        .bind(runner_id.to_string())
        .bind(lease_token)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to complete job with lease: {}", e)))?;
        Ok(result.rows_affected() == 1)
    }

    /// Persist an operator cancellation as a terminal job transition.
    pub async fn cancel(pool: &Pool, id: JobId, result_json: &str) -> Result<()> {
        let existing = Self::get(pool, id).await?;
        if let Some(job) = existing {
            if let Some(status) = JobStatus::from_str(&job.status) {
                if status.is_terminal() {
                    return Ok(());
                }
            }
        } else {
            return Err(Error::not_found("job", id));
        }
        sqlx::query(
            "UPDATE jobs SET status = 'cancelled', finished_at = ?, result_json = ? WHERE id = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(result_json)
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to cancel job: {}", e)))?;
        Ok(())
    }

    /// List jobs by pipeline run
    pub async fn list_by_run(
        pool: &Pool,
        run_id: PipelineRunId,
    ) -> Result<Vec<crate::models::Job>> {
        let rows =
            sqlx::query("SELECT * FROM jobs WHERE pipeline_run_id = ? ORDER BY created_at ASC")
                .bind(run_id.to_string())
                .fetch_all(pool.pool())
                .await
                .map_err(|e| Error::database(format!("failed to list jobs: {}", e)))?;

        let jobs = rows
            .into_iter()
            .map(|row| crate::models::Job {
                id: JobId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                pipeline_run_id: PipelineRunId::from(
                    Uuid::parse_str(&row.get::<String, _>("pipeline_run_id")).unwrap(),
                ),
                name: row.get("name"),
                status: row.get("status"),
                runner_id: row
                    .get::<Option<String>, _>("runner_id")
                    .and_then(|s| Uuid::parse_str(&s).ok().map(RunnerId::from)),
                started_at: row
                    .get::<Option<String>, _>("started_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                finished_at: row
                    .get::<Option<String>, _>("finished_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                retry_count: row.get("retry_count"),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
                commands: serde_json::from_str(
                    &row.get::<Option<String>, _>("commands")
                        .unwrap_or_else(|| "[]".to_string()),
                )
                .unwrap_or_default(),
                working_dir: row.get("working_dir"),
                result_json: row.get("result_json"),
            })
            .collect();

        Ok(jobs)
    }

    /// List all pending jobs
    pub async fn list_pending(pool: &Pool) -> Result<Vec<crate::models::Job>> {
        let rows = sqlx::query(
            "SELECT * FROM jobs WHERE status IN ('pending', 'queued') ORDER BY created_at ASC",
        )
        .fetch_all(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to list pending jobs: {}", e)))?;

        let jobs = rows
            .into_iter()
            .map(|row| crate::models::Job {
                id: JobId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                pipeline_run_id: PipelineRunId::from(
                    Uuid::parse_str(&row.get::<String, _>("pipeline_run_id")).unwrap(),
                ),
                name: row.get("name"),
                status: row.get("status"),
                runner_id: row
                    .get::<Option<String>, _>("runner_id")
                    .and_then(|s| Uuid::parse_str(&s).ok().map(RunnerId::from)),
                started_at: row
                    .get::<Option<String>, _>("started_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                finished_at: row
                    .get::<Option<String>, _>("finished_at")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                retry_count: row.get("retry_count"),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
                commands: serde_json::from_str(
                    &row.get::<Option<String>, _>("commands")
                        .unwrap_or_else(|| "[]".to_string()),
                )
                .unwrap_or_default(),
                working_dir: row.get("working_dir"),
                result_json: row.get("result_json"),
            })
            .collect();

        Ok(jobs)
    }

    /// Recover jobs that were in flight when the scheduler stopped. Assigned
    /// jobs have not started execution and are safe to requeue. Running jobs
    /// are fenced as failed instead of being re-run automatically: the old
    /// runner may still be alive, and requeueing would permit duplicate side
    /// effects without a durable runner-generation lease.
    pub async fn requeue_inflight(pool: &Pool) -> Result<u64> {
        let mut transaction = pool
            .pool()
            .begin()
            .await
            .map_err(|e| Error::database(format!("failed to begin recovery: {}", e)))?;
        let assigned = sqlx::query(
            "UPDATE jobs SET status = 'queued', runner_id = NULL, started_at = NULL, lease_token = NULL WHERE status = 'assigned'",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to requeue assigned jobs: {}", e)))?;
        let running = sqlx::query(
            "UPDATE jobs SET status = 'failed', runner_id = NULL, lease_token = NULL, finished_at = ?, result_json = ? WHERE status = 'running'",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(r#"{"status":"failed","reason":"scheduler_restart_fenced_running_job"}"#)
        .execute(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to fence running jobs: {}", e)))?;
        transaction
            .commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit recovery: {}", e)))?;
        Ok(assigned.rows_affected() + running.rows_affected())
    }
}

// ============================================================================
// Runner Queries
// ============================================================================

pub struct RunnerQueries;

impl RunnerQueries {
    /// Create a new runner
    pub async fn create(pool: &Pool, runner: &crate::models::Runner) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO runners (id, name, runner_type, status, capacity, labels, last_heartbeat, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(runner.id.to_string())
        .bind(&runner.name)
        .bind(&runner.runner_type)
        .bind(&runner.status)
        .bind(runner.capacity)
        .bind("[]") // labels as JSON array
        .bind(runner.last_heartbeat.map(|dt| dt.to_rfc3339()))
        .bind(runner.created_at.to_rfc3339())
        .bind(runner.created_at.to_rfc3339()) // updated_at same as created_at for new runner
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create runner: {}", e)))?;
        Ok(())
    }

    /// Get a runner by ID
    pub async fn get(pool: &Pool, id: RunnerId) -> Result<Option<crate::models::Runner>> {
        let row = sqlx::query("SELECT * FROM runners WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get runner: {}", e)))?;

        match row {
            Some(row) => Ok(Some(crate::models::Runner {
                id: RunnerId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                name: row.get("name"),
                runner_type: row.get("runner_type"),
                status: row.get("status"),
                capacity: row.get("capacity"),
                last_heartbeat: row
                    .get::<Option<String>, _>("last_heartbeat")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })),
            None => Ok(None),
        }
    }

    /// Update runner heartbeat
    pub async fn heartbeat(pool: &Pool, id: RunnerId) -> Result<()> {
        sqlx::query("UPDATE runners SET last_heartbeat = ? WHERE id = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to update heartbeat: {}", e)))?;
        Ok(())
    }

    /// Update runner status
    pub async fn update_status(pool: &Pool, id: RunnerId, status: &str) -> Result<()> {
        sqlx::query("UPDATE runners SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to update runner status: {}", e)))?;
        Ok(())
    }

    /// List all runners
    pub async fn list(pool: &Pool) -> Result<Vec<crate::models::Runner>> {
        let rows = sqlx::query("SELECT * FROM runners ORDER BY created_at DESC")
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list runners: {}", e)))?;

        let runners = rows
            .into_iter()
            .map(|row| crate::models::Runner {
                id: RunnerId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                name: row.get("name"),
                runner_type: row.get("runner_type"),
                status: row.get("status"),
                capacity: row.get("capacity"),
                last_heartbeat: row
                    .get::<Option<String>, _>("last_heartbeat")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(runners)
    }

    /// List online runners
    pub async fn list_online(pool: &Pool) -> Result<Vec<crate::models::Runner>> {
        let rows =
            sqlx::query("SELECT * FROM runners WHERE status = 'online' ORDER BY created_at DESC")
                .fetch_all(pool.pool())
                .await
                .map_err(|e| Error::database(format!("failed to list online runners: {}", e)))?;

        let runners = rows
            .into_iter()
            .map(|row| crate::models::Runner {
                id: RunnerId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
                name: row.get("name"),
                runner_type: row.get("runner_type"),
                status: row.get("status"),
                capacity: row.get("capacity"),
                last_heartbeat: row
                    .get::<Option<String>, _>("last_heartbeat")
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(runners)
    }
}

// ============================================================================
// Event Queries
// ============================================================================

pub struct EventQueries;

impl EventQueries {
    /// Create a new event
    pub async fn create(pool: &Pool, event: &crate::models::Event) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO events (id, event_type, payload, created_at)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(event.id.to_string())
        .bind(&event.event_type)
        .bind(event.payload.to_string())
        .bind(event.created_at.to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create event: {}", e)))?;
        Ok(())
    }

    /// List events by type
    pub async fn list_by_type(
        pool: &Pool,
        event_type: &str,
        limit: i64,
    ) -> Result<Vec<crate::models::Event>> {
        let rows = sqlx::query(
            "SELECT * FROM events WHERE event_type = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(event_type)
        .bind(limit)
        .fetch_all(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to list events: {}", e)))?;

        let events = rows
            .into_iter()
            .map(|row| crate::models::Event {
                id: Uuid::parse_str(&row.get::<String, _>("id")).unwrap(),
                event_type: row.get("event_type"),
                payload: serde_json::from_str(&row.get::<String, _>("payload")).unwrap_or_default(),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(events)
    }

    /// List recent events
    pub async fn list_recent(pool: &Pool, limit: i64) -> Result<Vec<crate::models::Event>> {
        let rows = sqlx::query("SELECT * FROM events ORDER BY created_at DESC LIMIT ?")
            .bind(limit)
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list events: {}", e)))?;

        let events = rows
            .into_iter()
            .map(|row| crate::models::Event {
                id: Uuid::parse_str(&row.get::<String, _>("id")).unwrap(),
                event_type: row.get("event_type"),
                payload: serde_json::from_str(&row.get::<String, _>("payload")).unwrap_or_default(),
                created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
                    .unwrap()
                    .with_timezone(&Utc),
            })
            .collect();

        Ok(events)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_repo_queries() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        // Create user first (repository has FK to owner)
        let user = crate::models::User::new(
            "owner".to_string(),
            "owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();

        let repo = crate::models::Repository::new(
            "test-repo".to_string(),
            user.id,
            "/git/test-repo".to_string(),
        );

        // Create
        RepoQueries::create(&pool, &repo).await.unwrap();

        // Get
        let found = RepoQueries::get(&pool, repo.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "test-repo");

        // List by owner
        let repos = RepoQueries::list_by_owner(&pool, user.id).await.unwrap();
        assert_eq!(repos.len(), 1);

        // List all
        let all_repos = RepoQueries::list(&pool).await.unwrap();
        assert_eq!(all_repos.len(), 1);

        // Delete
        RepoQueries::delete(&pool, repo.id).await.unwrap();
        let found = RepoQueries::get(&pool, repo.id).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_user_queries() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let user = crate::models::User::new(
            "testuser".to_string(),
            "test@example.com".to_string(),
            "hash".to_string(),
        );

        // Create
        UserQueries::create(&pool, &user).await.unwrap();

        // Get by ID
        let found = UserQueries::get(&pool, user.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().username, "testuser");

        // Get by username
        let found = UserQueries::get_by_username(&pool, "testuser")
            .await
            .unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().email, "test@example.com");

        // Not found
        let found = UserQueries::get_by_username(&pool, "nonexistent")
            .await
            .unwrap();
        assert!(found.is_none());

        // List all
        let all_users = UserQueries::list(&pool).await.unwrap();
        assert_eq!(all_users.len(), 1);
    }

    #[tokio::test]
    async fn test_runner_queries() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let runner = crate::models::Runner::new(
            "test-runner".to_string(),
            crate::models::RunnerType::Docker,
            2,
        );

        // Create
        RunnerQueries::create(&pool, &runner).await.unwrap();

        // Get
        let found = RunnerQueries::get(&pool, runner.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "test-runner");

        // List
        let runners = RunnerQueries::list(&pool).await.unwrap();
        assert_eq!(runners.len(), 1);

        // Heartbeat
        RunnerQueries::heartbeat(&pool, runner.id).await.unwrap();

        // Update status
        RunnerQueries::update_status(&pool, runner.id, "offline")
            .await
            .unwrap();
        let found = RunnerQueries::get(&pool, runner.id).await.unwrap();
        assert_eq!(found.unwrap().status, "offline");
    }

    #[tokio::test]
    async fn test_pipeline_queries() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        // Create user first
        let user = crate::models::User::new(
            "owner".to_string(),
            "owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();

        let repo = crate::models::Repository::new(
            "test-repo".to_string(),
            user.id,
            "/git/test-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();

        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "Test Pipeline".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };

        // Create
        PipelineQueries::create(&pool, &pipeline).await.unwrap();

        // Get
        let found = PipelineQueries::get(&pool, pipeline.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "Test Pipeline");

        // List by repo
        let pipelines = PipelineQueries::list_by_repo(&pool, repo.id).await.unwrap();
        assert_eq!(pipelines.len(), 1);

        // List all
        let all_pipelines = PipelineQueries::list(&pool).await.unwrap();
        assert_eq!(all_pipelines.len(), 1);
    }

    #[tokio::test]
    async fn test_pipeline_run_queries() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        // Create user first
        let user = crate::models::User::new(
            "owner".to_string(),
            "owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();

        let repo = crate::models::Repository::new(
            "test-repo".to_string(),
            user.id,
            "/git/test-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();

        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "Test Pipeline".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        PipelineQueries::create(&pool, &pipeline).await.unwrap();

        let run = crate::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "alice".to_string(),
            "abc123".to_string(),
        );

        // Create
        PipelineRunQueries::create(&pool, &run).await.unwrap();

        // Get
        let found = PipelineRunQueries::get(&pool, run.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().commit_hash, "abc123");

        // Update status
        PipelineRunQueries::update_status(&pool, run.id, "running")
            .await
            .unwrap();
        let found = PipelineRunQueries::get(&pool, run.id).await.unwrap();
        assert_eq!(found.unwrap().status, "running");

        PipelineRunQueries::update_status(&pool, run.id, "succeeded")
            .await
            .unwrap();
        let found = PipelineRunQueries::get(&pool, run.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, "succeeded");
        assert!(found.finished_at.is_some());

        // List by pipeline
        let runs = PipelineRunQueries::list_by_pipeline(&pool, pipeline.id)
            .await
            .unwrap();
        assert_eq!(runs.len(), 1);

        // List all
        let all_runs = PipelineRunQueries::list(&pool).await.unwrap();
        assert_eq!(all_runs.len(), 1);
    }

    #[tokio::test]
    async fn test_job_queries() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        // Create user first
        let user = crate::models::User::new(
            "owner".to_string(),
            "owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();

        let repo = crate::models::Repository::new(
            "test-repo".to_string(),
            user.id,
            "/git/test-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();

        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "Test Pipeline".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        PipelineQueries::create(&pool, &pipeline).await.unwrap();

        let run = crate::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "alice".to_string(),
            "abc123".to_string(),
        );
        PipelineRunQueries::create(&pool, &run).await.unwrap();

        let job = crate::models::Job::new(run.id, "build".to_string());

        // Create
        JobQueries::create(&pool, &job).await.unwrap();

        // Get
        let found = JobQueries::get(&pool, job.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "build");

        // Update status
        JobQueries::start(&pool, job.id).await.unwrap();
        let found = JobQueries::get(&pool, job.id).await.unwrap();
        assert_eq!(found.unwrap().status, "running");

        // Assign runner
        let runner = crate::models::Runner::new(
            "test-runner".to_string(),
            crate::models::RunnerType::Docker,
            2,
        );
        RunnerQueries::create(&pool, &runner).await.unwrap();
        JobQueries::assign(&pool, job.id, runner.id).await.unwrap();
        JobQueries::cancel(&pool, job.id, r#"{"status":"cancelled"}"#)
            .await
            .unwrap();
        let cancelled = JobQueries::get(&pool, job.id).await.unwrap().unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.finished_at.is_some());

        // List by run
        let jobs = JobQueries::list_by_run(&pool, run.id).await.unwrap();
        assert_eq!(jobs.len(), 1);
    }

    #[tokio::test]
    async fn test_job_queries_list_pending() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        // Create user, repo, pipeline, run, job
        let user = crate::models::User::new(
            "owner".to_string(),
            "owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();

        let repo = crate::models::Repository::new(
            "test-repo".to_string(),
            user.id,
            "/git/test-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();

        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "Test Pipeline".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        PipelineQueries::create(&pool, &pipeline).await.unwrap();

        let run = crate::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "alice".to_string(),
            "abc123".to_string(),
        );
        PipelineRunQueries::create(&pool, &run).await.unwrap();

        let job = crate::models::Job::new(run.id, "build".to_string());
        JobQueries::create(&pool, &job).await.unwrap();

        // List pending jobs
        let pending = JobQueries::list_pending(&pool).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "build");
    }

    #[tokio::test]
    async fn test_requeue_inflight_clears_assignment_and_start_state() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let user = crate::models::User::new(
            "recovery-owner".to_string(),
            "recovery@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();
        let repo = crate::models::Repository::new(
            "recovery-repo".to_string(),
            user.id,
            "/git/recovery".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();
        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "recovery-pipeline".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        PipelineQueries::create(&pool, &pipeline).await.unwrap();
        let run = crate::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "main".to_string(),
            "abc123".to_string(),
        );
        PipelineRunQueries::create(&pool, &run).await.unwrap();
        let job = crate::models::Job::new(run.id, "build".to_string());
        JobQueries::create(&pool, &job).await.unwrap();
        JobQueries::start(&pool, job.id).await.unwrap();

        assert_eq!(JobQueries::requeue_inflight(&pool).await.unwrap(), 1);
        let recovered = JobQueries::get(&pool, job.id).await.unwrap().unwrap();
        assert_eq!(recovered.status, "failed");
        assert!(recovered.runner_id.is_none());
        assert!(recovered.started_at.is_some());
        assert!(recovered
            .result_json
            .as_deref()
            .is_some_and(|receipt| receipt.contains("scheduler_restart_fenced_running_job")));
    }

    #[tokio::test]
    async fn test_runner_queries_list_online() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let runner = crate::models::Runner::new(
            "test-runner".to_string(),
            crate::models::RunnerType::Docker,
            2,
        );
        RunnerQueries::create(&pool, &runner).await.unwrap();

        // List online runners
        let online = RunnerQueries::list_online(&pool).await.unwrap();
        assert_eq!(online.len(), 1);
    }

    #[tokio::test]
    async fn test_event_queries_list_by_type_none() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let event = crate::models::Event::new(
            "push.received".to_string(),
            serde_json::json!({"repo": "test"}),
        );
        EventQueries::create(&pool, &event).await.unwrap();

        // List non-existent type
        let events = EventQueries::list_by_type(&pool, "nonexistent.type", 10)
            .await
            .unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_event_queries_list_recent_limit() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        // Create multiple events
        for i in 0..5 {
            let event = crate::models::Event::new(
                "push.received".to_string(),
                serde_json::json!({"repo": format!("test{}", i)}),
            );
            EventQueries::create(&pool, &event).await.unwrap();
        }

        // List with limit 3
        let recent = EventQueries::list_recent(&pool, 3).await.unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[tokio::test]
    async fn test_event_queries_empty() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        // List when no events
        let events = EventQueries::list_by_type(&pool, "push.received", 10)
            .await
            .unwrap();
        assert!(events.is_empty());

        let recent = EventQueries::list_recent(&pool, 10).await.unwrap();
        assert!(recent.is_empty());
    }

    #[tokio::test]
    async fn test_job_queries_get_nonexistent() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let found = JobQueries::get(&pool, JobId::new()).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_job_idempotency_reservation_is_stable() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let job_id = JobId::new();
        assert!(JobQueries::reserve_idempotency(
            &pool,
            "operator",
            "retry-1",
            "fingerprint",
            job_id
        )
        .await
        .unwrap());
        assert!(!JobQueries::reserve_idempotency(
            &pool,
            "operator",
            "retry-1",
            "fingerprint",
            JobId::new()
        )
        .await
        .unwrap());
        assert_eq!(
            JobQueries::get_idempotency(&pool, "operator", "retry-1")
                .await
                .unwrap()
                .unwrap(),
            (job_id, "fingerprint".to_string())
        );
    }

    #[tokio::test]
    async fn test_pipeline_queries_get_nonexistent() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let found = PipelineQueries::get(&pool, PipelineId::new())
            .await
            .unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_pipeline_run_queries_get_nonexistent() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let found = PipelineRunQueries::get(&pool, PipelineRunId::new())
            .await
            .unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_runner_queries_get_nonexistent() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let found = RunnerQueries::get(&pool, RunnerId::new()).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_user_queries_get_nonexistent() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let found = UserQueries::get(&pool, UserId::new()).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_user_role_defaults_to_developer() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = crate::models::User::new(
            "role-user".to_string(),
            "role@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();
        assert_eq!(
            UserQueries::get_role(&pool, user.id).await.unwrap(),
            Some("developer".to_string())
        );
    }

    #[tokio::test]
    async fn test_event_queries() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let event = crate::models::Event::new(
            "push.received".to_string(),
            serde_json::json!({"repo": "test"}),
        );

        // Create
        EventQueries::create(&pool, &event).await.unwrap();

        // List by type
        let events = EventQueries::list_by_type(&pool, "push.received", 10)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "push.received");

        // List recent
        let recent = EventQueries::list_recent(&pool, 10).await.unwrap();
        assert_eq!(recent.len(), 1);
    }
}
