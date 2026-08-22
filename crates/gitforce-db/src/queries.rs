//! Database queries implementation using SQLite
//!
//! This module provides real SQLite query implementations for all database operations.

use crate::Pool;
use chrono::{DateTime, Utc};
use gitforce_common::{Error, JobId, PipelineId, PipelineRunId, RepoId, Result, RunnerId, UserId};
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
}

// ============================================================================
// User Queries
// ============================================================================

pub struct UserQueries;

impl UserQueries {
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
        sqlx::query("UPDATE pipeline_runs SET status = ? WHERE id = ?")
            .bind(status)
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
    /// Create a new job
    pub async fn create(pool: &Pool, job: &crate::models::Job) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO jobs (id, pipeline_run_id, name, status, runner_id, started_at, finished_at, retry_count, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
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

    /// Assign a runner to a job
    pub async fn assign(pool: &Pool, id: JobId, runner_id: RunnerId) -> Result<()> {
        sqlx::query("UPDATE jobs SET runner_id = ? WHERE id = ?")
            .bind(runner_id.to_string())
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to assign job: {}", e)))?;
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
            })
            .collect();

        Ok(jobs)
    }

    /// List all pending jobs
    pub async fn list_pending(pool: &Pool) -> Result<Vec<crate::models::Job>> {
        let rows =
            sqlx::query("SELECT * FROM jobs WHERE status = 'pending' ORDER BY created_at ASC")
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
            })
            .collect();

        Ok(jobs)
    }
}

// ============================================================================
// Scheduler Job Queries
// ============================================================================

pub struct SchedulerJobQueries;

impl SchedulerJobQueries {
    /// Insert a scheduler job unless a row with the same ID already exists.
    ///
    /// Returns `true` when the row was inserted, `false` when a job with the
    /// same ID was already present (idempotent enqueue).
    pub async fn insert_if_absent(pool: &Pool, job: &crate::models::SchedulerJob) -> Result<bool> {
        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO scheduler_jobs (
                id, pipeline_run_id, repo_id, name, status, commands, working_dir,
                runner_id, claimed_at, success, exit_code, error, completed_at,
                receipt_id, requeue_count, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(job.id.to_string())
        .bind(job.pipeline_run_id.to_string())
        .bind(job.repo_id.to_string())
        .bind(&job.name)
        .bind(&job.status)
        .bind(serde_json::to_string(&job.commands).unwrap_or_else(|_| "[]".to_string()))
        .bind(&job.working_dir)
        .bind(job.runner_id.map(|id| id.to_string()))
        .bind(job.claimed_at.map(|dt| dt.to_rfc3339()))
        .bind(job.success)
        .bind(job.exit_code)
        .bind(&job.error)
        .bind(job.completed_at.map(|dt| dt.to_rfc3339()))
        .bind(&job.receipt_id)
        .bind(job.requeue_count)
        .bind(job.created_at.to_rfc3339())
        .bind(job.updated_at.to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to insert scheduler job: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }

    /// Get a scheduler job by ID
    pub async fn get(pool: &Pool, id: JobId) -> Result<Option<crate::models::SchedulerJob>> {
        let row = sqlx::query("SELECT * FROM scheduler_jobs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get scheduler job: {}", e)))?;

        Ok(row.map(|row| scheduler_job_from_row(&row)))
    }

    /// List scheduler jobs that still need attention (pending or claimed),
    /// oldest first.
    pub async fn list_active(pool: &Pool) -> Result<Vec<crate::models::SchedulerJob>> {
        let rows = sqlx::query(
            r#"
            SELECT * FROM scheduler_jobs
            WHERE status IN ('pending', 'claimed')
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to list active scheduler jobs: {}", e)))?;

        Ok(rows.iter().map(scheduler_job_from_row).collect())
    }

    /// Atomically move a pending scheduler job to claimed for a runner.
    ///
    /// Returns `true` when this call claimed the job, `false` when the job was
    /// missing or not pending (another writer got it first).
    pub async fn mark_claimed(pool: &Pool, id: JobId, runner_id: RunnerId) -> Result<bool> {
        let now = Utc::now();
        let result = sqlx::query(
            r#"
            UPDATE scheduler_jobs
            SET status = 'claimed', runner_id = ?, claimed_at = ?, updated_at = ?
            WHERE id = ? AND status = 'pending'
            "#,
        )
        .bind(runner_id.to_string())
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to claim scheduler job: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }

    /// Record the terminal outcome of a scheduler job.
    ///
    /// First terminal writer wins: the update only applies while the job is
    /// non-terminal. A new immutable receipt_id is minted for each winning
    /// terminal transition. Returns the stored terminal job — either written
    /// by this call or by an earlier writer — or `None` when the job does not
    /// exist.
    pub async fn complete(
        pool: &Pool,
        id: JobId,
        success: bool,
        exit_code: i64,
        error: Option<String>,
    ) -> Result<Option<crate::models::SchedulerJob>> {
        let now = Utc::now();
        let receipt_id = Uuid::new_v4().to_string();
        let result = sqlx::query(
            r#"
            UPDATE scheduler_jobs
            SET status = ?, success = ?, exit_code = ?, error = ?,
                completed_at = ?, updated_at = ?, receipt_id = ?
            WHERE id = ? AND status IN ('pending', 'claimed')
            "#,
        )
        .bind(if success { "succeeded" } else { "failed" })
        .bind(success)
        .bind(exit_code)
        .bind(&error)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(&receipt_id)
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to complete scheduler job: {}", e)))?;

        if result.rows_affected() == 0 {
            // Either unknown job or already terminal: fetch what is stored.
            // An unknown ID yields None; a terminal job yields the first
            // writer's record so the caller observes a stable outcome.
            return Self::get(pool, id).await;
        }

        Self::get(pool, id).await
    }

    /// Cancel a pending or claimed job. The first terminal writer wins.
    /// A new immutable receipt_id is minted for each winning transition.
    pub async fn cancel(pool: &Pool, id: JobId) -> Result<Option<crate::models::SchedulerJob>> {
        let now = Utc::now();
        let receipt_id = Uuid::new_v4().to_string();
        let result = sqlx::query(
            r#"
            UPDATE scheduler_jobs
            SET status = 'cancelled', success = 0, exit_code = -1,
                completed_at = ?, updated_at = ?, receipt_id = ?
            WHERE id = ? AND status IN ('pending', 'claimed')
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(&receipt_id)
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to cancel scheduler job: {}", e)))?;

        if result.rows_affected() == 0 {
            return Self::get(pool, id).await;
        }
        Self::get(pool, id).await
    }

    /// Return a claimed scheduler job to the pending pool (e.g. the runner
    /// disappeared before reporting a result).
    ///
    /// Returns `true` when the job was requeued, `false` when it was not in
    /// the claimed state.
    pub async fn requeue_claimed(pool: &Pool, id: JobId) -> Result<bool> {
        let now = Utc::now();
        let result = sqlx::query(
            r#"
            UPDATE scheduler_jobs
            SET status = 'pending', runner_id = NULL, claimed_at = NULL, updated_at = ?
            WHERE id = ? AND status = 'claimed'
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to requeue scheduler job: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }

    /// Reap stale claimed jobs whose lease has expired.
    ///
    /// For each stale claimed job (claimed_at older than `lease_secs`):
    ///   - If requeue_count < max_retries: atomically requeue (status='pending',
    ///     clear runner_id/claimed_at, increment requeue_count, update updated_at).
    ///     The job will be handed out again on restart because it is pending.
    ///   - If requeue_count >= max_retries: atomically transition to 'failed'
    ///     with a durable receipt_id and clear claim state.
    ///
    /// Returns the count of jobs that were transitioned (requeued + failed).
    /// Jobs with a terminal status at the time of the UPDATE are skipped
    /// (first-terminal-writer-wins semantics).
    pub async fn reap_stale_claimed(pool: &Pool, lease_secs: i64, max_retries: i32) -> Result<i32> {
        let now = Utc::now();
        let cutoff = now - chrono::Duration::seconds(lease_secs);

        // Select stale claimed jobs ordered by creation time (oldest first).
        let rows = sqlx::query(
            r#"
            SELECT id FROM scheduler_jobs
            WHERE status = 'claimed'
              AND claimed_at IS NOT NULL
              AND claimed_at < ?
            ORDER BY created_at ASC
            "#,
        )
        .bind(cutoff.to_rfc3339())
        .fetch_all(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to select stale claimed jobs: {}", e)))?;

        let mut transitioned = 0;
        for row in rows {
            let id: String = row.get("id");
            let job_id = JobId::from(Uuid::parse_str(&id).unwrap());

            // Fetch current state to decide whether to requeue or fail.
            let current = Self::get(pool, job_id).await?;
            let current = match current {
                Some(j) => j,
                None => continue,
            };

            // Skip if already terminal.
            if current.is_terminal() {
                continue;
            }

            // Determine the requeue count from the row we just read.
            let requeue_count = current.requeue_count;

            if requeue_count >= max_retries {
                // Transition to failed: first-terminal-writer wins via WHERE
                // status IN ('pending','claimed'), mint fresh receipt_id.
                let fail_receipt = Uuid::new_v4().to_string();
                let result = sqlx::query(
                    r#"
                    UPDATE scheduler_jobs
                    SET status = 'failed', success = 0, exit_code = -2,
                        error = 'reaped: lease expired and retry cap exceeded',
                        completed_at = ?, updated_at = ?, receipt_id = ?,
                        runner_id = NULL, claimed_at = NULL
                    WHERE id = ? AND status IN ('pending', 'claimed')
                    "#,
                )
                .bind(now.to_rfc3339())
                .bind(now.to_rfc3339())
                .bind(&fail_receipt)
                .bind(&id)
                .execute(pool.pool())
                .await
                .map_err(|e| {
                    Error::database(format!("failed to reap scheduler job to failed: {}", e))
                })?;

                if result.rows_affected() > 0 {
                    transitioned += 1;
                }
            } else {
                // Requeue: clear claim state, bump requeue_count.
                let result = sqlx::query(
                    r#"
                    UPDATE scheduler_jobs
                    SET status = 'pending', runner_id = NULL, claimed_at = NULL,
                        requeue_count = requeue_count + 1, updated_at = ?
                    WHERE id = ? AND status = 'claimed'
                    "#,
                )
                .bind(now.to_rfc3339())
                .bind(&id)
                .execute(pool.pool())
                .await
                .map_err(|e| {
                    Error::database(format!("failed to reap scheduler job to pending: {}", e))
                })?;

                if result.rows_affected() > 0 {
                    transitioned += 1;
                }
            }
        }

        Ok(transitioned)
    }
}

/// Map a scheduler_jobs row to the model
fn scheduler_job_from_row(row: &sqlx::sqlite::SqliteRow) -> crate::models::SchedulerJob {
    crate::models::SchedulerJob {
        id: JobId::from(Uuid::parse_str(&row.get::<String, _>("id")).unwrap()),
        pipeline_run_id: PipelineRunId::from(
            Uuid::parse_str(&row.get::<String, _>("pipeline_run_id")).unwrap(),
        ),
        repo_id: RepoId::from(Uuid::parse_str(&row.get::<String, _>("repo_id")).unwrap()),
        name: row.get("name"),
        status: row.get("status"),
        commands: serde_json::from_str(&row.get::<String, _>("commands")).unwrap_or_default(),
        working_dir: row.get("working_dir"),
        runner_id: row
            .get::<Option<String>, _>("runner_id")
            .and_then(|s| Uuid::parse_str(&s).ok().map(RunnerId::from)),
        claimed_at: row
            .get::<Option<String>, _>("claimed_at")
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc)),
        success: row.get("success"),
        exit_code: row.get("exit_code"),
        error: row.get("error"),
        completed_at: row
            .get::<Option<String>, _>("completed_at")
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc)),
        receipt_id: row.get("receipt_id"),
        requeue_count: row.get::<i32, _>("requeue_count"),
        created_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("created_at"))
            .unwrap()
            .with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(&row.get::<String, _>("updated_at"))
            .unwrap()
            .with_timezone(&Utc),
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
        JobQueries::update_status(&pool, job.id, "running")
            .await
            .unwrap();
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

    // ------------------------------------------------------------------------
    // Scheduler job queries
    // ------------------------------------------------------------------------

    fn sample_scheduler_job(name: &str) -> crate::models::SchedulerJob {
        crate::models::SchedulerJob::new(
            PipelineRunId::new(),
            RepoId::new(),
            name.to_string(),
            vec!["make build".to_string(), "make test".to_string()],
        )
        .with_working_dir(Some("/workspace/repo".to_string()))
    }

    #[tokio::test]
    async fn test_scheduler_job_round_trip() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");

        // Insert
        let inserted = SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        assert!(inserted);

        // Round-trip: every field survives the write/read cycle
        let found = SchedulerJobQueries::get(&pool, job.id).await.unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.id, job.id);
        assert_eq!(found.pipeline_run_id, job.pipeline_run_id);
        assert_eq!(found.repo_id, job.repo_id);
        assert_eq!(found.name, "build");
        assert_eq!(found.status, "pending");
        assert_eq!(
            found.commands,
            vec!["make build".to_string(), "make test".to_string()]
        );
        assert_eq!(found.working_dir.as_deref(), Some("/workspace/repo"));
        assert!(found.runner_id.is_none());
        assert!(found.claimed_at.is_none());
        assert!(found.success.is_none());
        assert!(found.exit_code.is_none());
        assert!(found.error.is_none());
        assert!(found.completed_at.is_none());
        assert!(found.receipt_id.is_none());
        assert_eq!(found.created_at, job.created_at);
        assert_eq!(found.updated_at, job.updated_at);
        assert!(found.is_active());

        // Not found for a random ID
        let missing = SchedulerJobQueries::get(&pool, JobId::new()).await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_job_duplicate_insert_is_ignored() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");
        assert!(SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap());

        // Same ID inserted again is a no-op, not an error
        let mut duplicate = sample_scheduler_job("changed");
        duplicate.id = job.id;
        duplicate.name = "changed".to_string();
        let inserted = SchedulerJobQueries::insert_if_absent(&pool, &duplicate)
            .await
            .unwrap();
        assert!(!inserted);

        // The original row is untouched
        let found = SchedulerJobQueries::get(&pool, job.id).await.unwrap();
        assert_eq!(found.unwrap().name, "build");

        // list_active still sees exactly one row
        let active = SchedulerJobQueries::list_active(&pool).await.unwrap();
        assert_eq!(active.len(), 1);
    }

    #[tokio::test]
    async fn test_scheduler_job_list_active_and_claim() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job1 = sample_scheduler_job("build");
        let job2 = sample_scheduler_job("test");
        SchedulerJobQueries::insert_if_absent(&pool, &job1)
            .await
            .unwrap();
        SchedulerJobQueries::insert_if_absent(&pool, &job2)
            .await
            .unwrap();

        // Both pending jobs are listed as active
        let active = SchedulerJobQueries::list_active(&pool).await.unwrap();
        assert_eq!(active.len(), 2);
        assert!(active.iter().any(|job| job.id == job1.id));
        assert!(active.iter().any(|job| job.id == job2.id));

        // Claim job1 for a runner
        let runner_id = RunnerId::new();
        let claimed = SchedulerJobQueries::mark_claimed(&pool, job1.id, runner_id)
            .await
            .unwrap();
        assert!(claimed);

        let found = SchedulerJobQueries::get(&pool, job1.id).await.unwrap();
        let found = found.unwrap();
        assert_eq!(found.status, "claimed");
        assert_eq!(found.runner_id, Some(runner_id));
        assert!(found.claimed_at.is_some());
        assert!(found.is_active());

        // A claimed job is still active
        let active = SchedulerJobQueries::list_active(&pool).await.unwrap();
        assert_eq!(active.len(), 2);

        // Double-claim is rejected: the job is no longer pending
        let second_claim = SchedulerJobQueries::mark_claimed(&pool, job1.id, RunnerId::new())
            .await
            .unwrap();
        assert!(!second_claim);
        let found = SchedulerJobQueries::get(&pool, job1.id).await.unwrap();
        assert_eq!(found.unwrap().runner_id, Some(runner_id));

        // Claiming an unknown job is a no-op
        let unknown_claim = SchedulerJobQueries::mark_claimed(&pool, JobId::new(), RunnerId::new())
            .await
            .unwrap();
        assert!(!unknown_claim);
    }

    #[tokio::test]
    async fn test_scheduler_job_completion_is_idempotent() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();

        // First writer completes the job
        let first = SchedulerJobQueries::complete(
            &pool,
            job.id,
            false,
            2,
            Some("build failed".to_string()),
        )
        .await
        .unwrap()
        .expect("job should exist");
        assert_eq!(first.status, "failed");
        assert_eq!(first.success, Some(false));
        assert_eq!(first.exit_code, Some(2));
        assert_eq!(first.error.as_deref(), Some("build failed"));
        assert!(first.completed_at.is_some());
        assert!(first.is_terminal());
        assert!(!first.is_active());
        assert!(first.receipt_id.is_some());

        // Terminal jobs drop out of the active list
        let active = SchedulerJobQueries::list_active(&pool).await.unwrap();
        assert!(active.is_empty());

        // Second writer with a different outcome must not overwrite
        let second = SchedulerJobQueries::complete(&pool, job.id, true, 0, None)
            .await
            .unwrap()
            .expect("job should exist");
        assert_eq!(second.status, "failed");
        assert_eq!(second.success, Some(false));
        assert_eq!(second.exit_code, Some(2));
        assert_eq!(second.error.as_deref(), Some("build failed"));
        assert_eq!(second.completed_at, first.completed_at);
        // Receipt_id is immutable — the replay returns the first writer's receipt.
        assert_eq!(second.receipt_id, first.receipt_id);
    }

    #[tokio::test]
    async fn test_scheduler_job_unknown_completion_returns_none() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let unknown = SchedulerJobQueries::complete(&pool, JobId::new(), true, 0, None)
            .await
            .unwrap();
        assert!(unknown.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_job_cancel_is_terminal_and_idempotent() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("cancel-me");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        let cancelled = SchedulerJobQueries::cancel(&pool, job.id)
            .await
            .unwrap()
            .expect("job should exist");
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(cancelled.success, Some(false));
        assert_eq!(cancelled.exit_code, Some(-1));
        assert!(cancelled.is_terminal());
        assert!(cancelled.receipt_id.is_some());
        assert!(SchedulerJobQueries::list_active(&pool)
            .await
            .unwrap()
            .is_empty());

        let second = SchedulerJobQueries::cancel(&pool, job.id)
            .await
            .unwrap()
            .expect("cancelled job should remain observable");
        assert_eq!(second.status, "cancelled");
        assert_eq!(second.completed_at, cancelled.completed_at);
        // Receipt_id is immutable on replay.
        assert_eq!(second.receipt_id, cancelled.receipt_id);
        assert!(SchedulerJobQueries::cancel(&pool, JobId::new())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_scheduler_job_requeue_claimed() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();

        // Requeueing a pending (not claimed) job is a no-op
        let requeued = SchedulerJobQueries::requeue_claimed(&pool, job.id)
            .await
            .unwrap();
        assert!(!requeued);

        // Claim, then requeue
        let runner_id = RunnerId::new();
        SchedulerJobQueries::mark_claimed(&pool, job.id, runner_id)
            .await
            .unwrap();
        let requeued = SchedulerJobQueries::requeue_claimed(&pool, job.id)
            .await
            .unwrap();
        assert!(requeued);

        let found = SchedulerJobQueries::get(&pool, job.id).await.unwrap();
        let found = found.unwrap();
        assert_eq!(found.status, "pending");
        assert!(found.runner_id.is_none());
        assert!(found.claimed_at.is_none());
        assert!(found.is_active());

        // Requeued job can be claimed again
        let claimed = SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();
        assert!(claimed);

        // Requeueing an unknown job is a no-op
        let unknown = SchedulerJobQueries::requeue_claimed(&pool, JobId::new())
            .await
            .unwrap();
        assert!(!unknown);
    }

    // ------------------------------------------------------------------------
    // Receipt_id tests
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_scheduler_job_success_completion_mints_receipt_id() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();

        let completed = SchedulerJobQueries::complete(&pool, job.id, true, 0, None)
            .await
            .unwrap()
            .expect("job should exist");

        assert_eq!(completed.status, "succeeded");
        assert!(completed.receipt_id.is_some());
        let receipt = completed.receipt_id.clone();

        // Replay must return the same receipt_id.
        let replay = SchedulerJobQueries::complete(&pool, job.id, false, 1, Some("oops".into()))
            .await
            .unwrap()
            .expect("job still exists");
        assert_eq!(replay.receipt_id, receipt);
    }

    #[tokio::test]
    async fn test_scheduler_job_failure_completion_mints_receipt_id() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();

        let completed =
            SchedulerJobQueries::complete(&pool, job.id, false, 42, Some("segfault".to_string()))
                .await
                .unwrap()
                .expect("job should exist");

        assert_eq!(completed.status, "failed");
        assert!(completed.receipt_id.is_some());
        let receipt = completed.receipt_id.clone();

        // Replay must not overwrite.
        let replay = SchedulerJobQueries::complete(&pool, job.id, true, 0, None)
            .await
            .unwrap()
            .expect("job still exists");
        assert_eq!(replay.receipt_id, receipt);
        assert_eq!(replay.status, "failed");
    }

    #[tokio::test]
    async fn test_scheduler_job_cancel_mints_receipt_id() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();

        let cancelled = SchedulerJobQueries::cancel(&pool, job.id)
            .await
            .unwrap()
            .expect("job should exist");

        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.receipt_id.is_some());
        let receipt = cancelled.receipt_id.clone();

        // Replay must return the same receipt_id.
        let replay = SchedulerJobQueries::cancel(&pool, job.id)
            .await
            .unwrap()
            .expect("job still exists");
        assert_eq!(replay.receipt_id, receipt);
    }

    #[tokio::test]
    async fn test_scheduler_job_conflicting_terminal_writers() {
        // Two callers race to terminal-write the same job; the first writer's
        // receipt must be stable and the second must not overwrite it.
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("race");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();

        // Caller A wins with "succeeded".
        let winner_a = SchedulerJobQueries::complete(&pool, job.id, true, 0, None)
            .await
            .unwrap()
            .expect("job exists");

        // Caller B loses with "failed"; must observe winner A's outcome.
        let loser_b =
            SchedulerJobQueries::complete(&pool, job.id, false, 1, Some("i lost".to_string()))
                .await
                .unwrap()
                .expect("job still exists");

        assert_eq!(loser_b.status, "succeeded");
        assert_eq!(loser_b.receipt_id, winner_a.receipt_id);
        assert!(loser_b.error.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_job_receipt_id_persists_across_restart() {
        // Simulate a scheduler restart: a job is completed, then a new pool
        // reads the same row from disk and must observe the same receipt_id.
        let db_path = std::env::var("HOME")
            .map(|home| {
                format!(
                    "{home}/.cache/gitforge-receipt-test-{}-{}.db",
                    std::process::id(),
                    uuid::Uuid::new_v4()
                )
            })
            .expect("HOME must be set for restart test");
        let pool = Pool::new(&db_path).await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();

        let before = SchedulerJobQueries::complete(&pool, job.id, true, 0, None)
            .await
            .unwrap()
            .expect("job exists");
        let receipt_before = before.receipt_id.clone().expect("receipt_id set");

        // Simulate restart: a new pool opens the same durable file.
        let pool2 = Pool::new(&db_path).await.unwrap();
        pool2.migrate().await.unwrap();

        let after = SchedulerJobQueries::get(&pool2, job.id)
            .await
            .unwrap()
            .expect("job row still exists");
        assert_eq!(after.receipt_id, Some(receipt_before));
        drop(pool2);
        drop(pool);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn test_scheduler_job_pending_job_has_no_receipt_id() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();

        let found = SchedulerJobQueries::get(&pool, job.id)
            .await
            .unwrap()
            .expect("job exists");
        assert!(found.receipt_id.is_none());
        assert!(found.completed_at.is_none());
    }

    // ------------------------------------------------------------------------
    // Reaper tests
    // ------------------------------------------------------------------------

    /// Helper: advance the claimed_at timestamp of a job directly in the DB
    /// so it appears older than the lease threshold.
    async fn make_claimed_job_stale(pool: &Pool, job_id: JobId, lease_secs: i64) {
        let stale_time = Utc::now() - chrono::Duration::seconds(lease_secs + 10);
        sqlx::query("UPDATE scheduler_jobs SET claimed_at = ? WHERE id = ?")
            .bind(stale_time.to_rfc3339())
            .bind(job_id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_reaper_fresh_claim_not_requeued() {
        // A recently-claimed job (within lease) must not be touched.
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("fresh");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();

        // lease=60, job was just claimed — nothing to reap.
        let n = SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3)
            .await
            .unwrap();
        assert_eq!(n, 0);

        // Job is still claimed.
        let found = SchedulerJobQueries::get(&pool, job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, "claimed");
        assert_eq!(found.requeue_count, 0);
    }

    #[tokio::test]
    async fn test_reaper_stale_claim_requeued_once() {
        // A stale claimed job below the retry cap is requeued.
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("stale-requeue");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();
        make_claimed_job_stale(&pool, job.id, 60).await;

        let n = SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3)
            .await
            .unwrap();
        assert_eq!(n, 1);

        let found = SchedulerJobQueries::get(&pool, job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, "pending");
        assert!(found.runner_id.is_none());
        assert!(found.claimed_at.is_none());
        assert_eq!(found.requeue_count, 1);
        assert!(found.is_active()); // pending is active
    }

    #[tokio::test]
    async fn test_reaper_cap_becomes_explicit_failure() {
        // A stale claimed job at or above the retry cap transitions to failed.
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("cap-fail");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();

        // Manually set requeue_count to max_retries (3) to be at/above cap.
        sqlx::query("UPDATE scheduler_jobs SET requeue_count = 3 WHERE id = ?")
            .bind(job.id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();

        make_claimed_job_stale(&pool, job.id, 60).await;

        // max_retries=3, current requeue_count=3 → >= cap → fails.
        let n = SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3)
            .await
            .unwrap();
        assert_eq!(n, 1);

        let found = SchedulerJobQueries::get(&pool, job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, "failed");
        assert!(found.is_terminal());
        assert!(found.receipt_id.is_some());
        assert!(found.runner_id.is_none()); // claim state cleared
        assert!(found.claimed_at.is_none());
        assert_eq!(found.exit_code, Some(-2));
        assert!(found.error.as_ref().is_some_and(|e| e.contains("reaped")));
    }

    #[tokio::test]
    async fn test_reaper_restart_does_not_strand_stale_claims() {
        // After requeue, a scheduler restart must see the job as pending
        // (not stuck in claimed), so it can be handed out again.
        let db_path = std::env::var("HOME")
            .map(|home| {
                format!(
                    "{home}/.cache/gitforge-reaper-restart-{}-{}.db",
                    std::process::id(),
                    uuid::Uuid::new_v4()
                )
            })
            .unwrap();
        let pool = Pool::new(&db_path).await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("restart-strand");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();
        make_claimed_job_stale(&pool, job.id, 60).await;

        // Reaper requeues the stale job.
        let n = SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3)
            .await
            .unwrap();
        assert_eq!(n, 1);

        // Simulate restart: new pool opens the same file.
        drop(pool);
        let pool2 = Pool::new(&db_path).await.unwrap();
        pool2.migrate().await.unwrap();

        // After restart the job must appear as pending (not claimed).
        let found = SchedulerJobQueries::get(&pool2, job.id)
            .await
            .unwrap()
            .expect("job row still exists");
        assert_eq!(found.status, "pending");
        assert_eq!(found.requeue_count, 1);
        assert!(found.is_active()); // pending is active

        // The requeued job can be claimed again after restart.
        let re_claimed = SchedulerJobQueries::mark_claimed(&pool2, job.id, RunnerId::new())
            .await
            .unwrap();
        assert!(re_claimed);

        drop(pool2);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn test_reaper_concurrent_calls_do_not_duplicate() {
        // Two concurrent reaper calls must not double-transition the same job.
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("concurrent-reap");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();
        make_claimed_job_stale(&pool, job.id, 60).await;

        // Fire two reaper calls concurrently.
        let (r1, r2) = tokio::join!(
            SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3),
            SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3),
        );

        let n1 = r1.unwrap();
        let n2 = r2.unwrap();

        // One of them must have transitioned the job, the other zero.
        assert_eq!(n1 + n2, 1, "exactly one reaper call should transition");

        // Job is pending and requeue_count is exactly 1.
        let found = SchedulerJobQueries::get(&pool, job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, "pending");
        assert_eq!(found.requeue_count, 1);
    }

    #[tokio::test]
    async fn test_reaper_unknown_job_is_noop() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let n = SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn test_reaper_already_terminal_skipped() {
        // A claimed job that becomes terminal before the reaper runs must be skipped.
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("terminal-before-reap");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();

        // Manually expire it so it looks stale.
        make_claimed_job_stale(&pool, job.id, 60).await;

        // Complete it externally before the reaper runs (simulating a runner
        // that reported success just before the reaper window).
        SchedulerJobQueries::complete(&pool, job.id, true, 0, None)
            .await
            .unwrap();

        // Reaper must not overwrite the terminal state.
        let n = SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3)
            .await
            .unwrap();
        assert_eq!(n, 0);

        let found = SchedulerJobQueries::get(&pool, job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, "succeeded");
        assert!(found.receipt_id.is_some()); // original receipt preserved
    }

    #[tokio::test]
    async fn test_reaper_multiple_stale_jobs_mixed_outcomes() {
        // Five stale jobs: two below cap (→ requeued), two at cap (→ failed), one fresh (→ untouched).
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job_fresh = sample_scheduler_job("fresh");
        let job_reap1 = sample_scheduler_job("reap1");
        let job_reap2 = sample_scheduler_job("reap2");
        let job_fail1 = sample_scheduler_job("fail1");
        let job_fail2 = sample_scheduler_job("fail2");

        for job in [&job_fresh, &job_reap1, &job_reap2, &job_fail1, &job_fail2] {
            SchedulerJobQueries::insert_if_absent(&pool, job)
                .await
                .unwrap();
        }

        // Claim all.
        SchedulerJobQueries::mark_claimed(&pool, job_fresh.id, RunnerId::new())
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job_reap1.id, RunnerId::new())
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job_reap2.id, RunnerId::new())
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job_fail1.id, RunnerId::new())
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job_fail2.id, RunnerId::new())
            .await
            .unwrap();

        // Expire the four non-fresh ones.
        make_claimed_job_stale(&pool, job_reap1.id, 60).await;
        make_claimed_job_stale(&pool, job_reap2.id, 60).await;
        make_claimed_job_stale(&pool, job_fail1.id, 60).await;
        make_claimed_job_stale(&pool, job_fail2.id, 60).await;

        // Set fail jobs to be at/above cap (>= max_retries=3).
        sqlx::query("UPDATE scheduler_jobs SET requeue_count = 3 WHERE id = ? OR id = ?")
            .bind(job_fail1.id.to_string())
            .bind(job_fail2.id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        let n = SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3)
            .await
            .unwrap();
        // 2 requeued + 2 failed = 4 transitioned (fresh is within lease).
        assert_eq!(n, 4);

        let fresh = SchedulerJobQueries::get(&pool, job_fresh.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fresh.status, "claimed"); // untouched

        for (job, expected_status) in [
            (job_reap1, "pending"),
            (job_reap2, "pending"),
            (job_fail1, "failed"),
            (job_fail2, "failed"),
        ] {
            let found = SchedulerJobQueries::get(&pool, job.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(found.status, expected_status);
        }
    }

    #[tokio::test]
    async fn test_reaper_receipt_id_first_terminal_writer_wins() {
        // A stale job that completes before reaper runs: reaper must not overwrite
        // the terminal receipt_id.
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("receipt-reap");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();
        make_claimed_job_stale(&pool, job.id, 60).await;

        // Complete it externally first.
        let completed = SchedulerJobQueries::complete(&pool, job.id, true, 0, None)
            .await
            .unwrap()
            .expect("job exists");
        let original_receipt = completed.receipt_id.clone().expect("receipt set");

        // Reaper must not transition it.
        let n = SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3)
            .await
            .unwrap();
        assert_eq!(n, 0);

        let found = SchedulerJobQueries::get(&pool, job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, "succeeded");
        assert_eq!(found.receipt_id, Some(original_receipt)); // immutable
    }

    // ------------------------------------------------------------------------
    // requeue_count field tests
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn test_scheduler_job_requeue_count_defaults_to_zero() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();

        let found = SchedulerJobQueries::get(&pool, job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.requeue_count, 0);
    }

    #[tokio::test]
    async fn test_scheduler_job_requeue_count_increments_on_reap() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let job = sample_scheduler_job("build");
        SchedulerJobQueries::insert_if_absent(&pool, &job)
            .await
            .unwrap();
        SchedulerJobQueries::mark_claimed(&pool, job.id, RunnerId::new())
            .await
            .unwrap();
        make_claimed_job_stale(&pool, job.id, 60).await;

        SchedulerJobQueries::reap_stale_claimed(&pool, 60, 3)
            .await
            .unwrap();

        let found = SchedulerJobQueries::get(&pool, job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.requeue_count, 1);
    }
}
