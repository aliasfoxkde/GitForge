//! Database queries implementation using SQLite
//!
//! This module provides real SQLite query implementations for all database operations.

use crate::connection::begin_immediate;
use crate::models::JobStatus;
use crate::Pool;
use chrono::{DateTime, Utc};
use gitforge_common::{
    Error, JobId, PipelineId, PipelineRunId, RepoId, Result, RunnerId, SshKeyId, UserId,
};
use sqlx::Row;
use uuid::Uuid;

fn parse_uuid_column(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Uuid> {
    let value: String = row
        .try_get(column)
        .map_err(|error| Error::database(format!("missing {column} column: {error}")))?;
    Uuid::parse_str(&value)
        .map_err(|error| Error::database(format!("invalid UUID in {column}: {error}")))
}

/// How hard a durable write fights SQLite write-lock contention before
/// giving up. The live instance has recorded 19-40 s COMMIT stalls under
/// co-tenant load; the 30 s busy timeout alone therefore leaves a failure
/// tail, and interactive paths sitting on top of a one-shot write (the API
/// cancel endpoint 500-ing "Failed to persist cancellation" through a
/// storm) turn that tail into user-visible errors. Five attempts with
/// exponential backoff cap the added latency well under the busy timeout's
/// own worst case while covering the bulk of the storm window.
const PERSIST_ATTEMPTS: usize = 5;
const PERSIST_BACKOFF_BASE: std::time::Duration = std::time::Duration::from_millis(250);
const PERSIST_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(4);

/// Run a durable write until it commits or the retry budget is exhausted.
///
/// This is the shared persistence discipline for one-shot writes that must
/// not surface `database is busy` to callers (the F21/F23 class). Only
/// database errors are retried — under contention the distinguishing
/// feature of a busy failure is that a retry succeeds — while validation
/// and not-found errors propagate immediately: retrying them cannot change
/// the outcome.
pub async fn persist_with_retry<T, F, Fut>(mut operation: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_error = None;
    for attempt in 0..PERSIST_ATTEMPTS {
        if attempt > 0 {
            let backoff = PERSIST_BACKOFF_BASE
                .saturating_mul(1 << (attempt - 1).min(4))
                .min(PERSIST_BACKOFF_CAP);
            tokio::time::sleep(backoff).await;
        }
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if error.kind != gitforge_common::ErrorKind::Database {
                    return Err(error);
                }
                tracing::warn!(
                    attempt = attempt + 1,
                    %error,
                    "durable write hit a transient failure; retrying"
                );
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| Error::database("durable write made no attempts")))
}

fn parse_timestamp_column(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<DateTime<Utc>> {
    let value: String = row
        .try_get(column)
        .map_err(|error| Error::database(format!("missing {column} column: {error}")))?;
    DateTime::parse_from_rfc3339(&value)
        .map(|date| date.with_timezone(&Utc))
        .map_err(|error| Error::database(format!("invalid timestamp in {column}: {error}")))
}

fn parse_optional_timestamp_column(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<Option<DateTime<Utc>>> {
    let value: Option<String> = row
        .try_get(column)
        .map_err(|error| Error::database(format!("invalid {column} column: {error}")))?;
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|date| date.with_timezone(&Utc))
                .map_err(|error| Error::database(format!("invalid timestamp in {column}: {error}")))
        })
        .transpose()
}

fn hydrate_pipeline(row: sqlx::sqlite::SqliteRow) -> Result<crate::models::Pipeline> {
    Ok(crate::models::Pipeline {
        id: PipelineId::from(parse_uuid_column(&row, "id")?),
        repo_id: RepoId::from(parse_uuid_column(&row, "repo_id")?),
        name: row
            .try_get("name")
            .map_err(|error| Error::database(format!("invalid pipeline name: {error}")))?,
        trigger_type: row
            .try_get("trigger_type")
            .map_err(|error| Error::database(format!("invalid pipeline trigger type: {error}")))?,
        config: serde_json::from_str(
            &row.try_get::<String, _>("config")
                .map_err(|error| Error::database(format!("invalid pipeline config: {error}")))?,
        )
        .map_err(|error| Error::database(format!("invalid pipeline config JSON: {error}")))?,
        created_at: parse_timestamp_column(&row, "created_at")?,
    })
}

fn hydrate_repository(row: sqlx::sqlite::SqliteRow) -> Result<crate::models::Repository> {
    Ok(crate::models::Repository {
        id: RepoId::from(parse_uuid_column(&row, "id")?),
        name: row
            .try_get("name")
            .map_err(|error| Error::database(format!("invalid repository name: {error}")))?,
        owner_id: UserId::from(parse_uuid_column(&row, "owner_id")?),
        visibility: row
            .try_get("visibility")
            .map_err(|error| Error::database(format!("invalid repository visibility: {error}")))?,
        git_path: row
            .try_get("git_path")
            .map_err(|error| Error::database(format!("invalid repository git path: {error}")))?,
        required_checks: parse_required_checks_column(&row)?,
        deny_non_fast_forward: row
            .try_get::<i64, _>("deny_non_fast_forward")
            .map(|value| value != 0)
            .map_err(|error| {
                Error::database(format!("invalid repository deny_non_fast_forward: {error}"))
            })?,
        created_at: parse_timestamp_column(&row, "created_at")?,
        updated_at: parse_timestamp_column(&row, "updated_at")?,
    })
}

/// Decode the `required_checks` JSON column into pipeline names. A NULL
/// (pre-migration row) or malformed payload degrades to no required checks
/// rather than poisoning every repository read.
fn parse_required_checks_column(row: &sqlx::sqlite::SqliteRow) -> Result<Vec<String>> {
    let raw: Option<String> = row
        .try_get("required_checks")
        .map_err(|error| Error::database(format!("invalid repository required_checks: {error}")))?;
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&raw).map_err(|error| {
        Error::database(format!("invalid repository required_checks JSON: {error}"))
    })
}

fn hydrate_user(row: sqlx::sqlite::SqliteRow) -> Result<crate::models::User> {
    Ok(crate::models::User {
        id: UserId::from(parse_uuid_column(&row, "id")?),
        username: row
            .try_get("username")
            .map_err(|error| Error::database(format!("invalid username: {error}")))?,
        email: row
            .try_get("email")
            .map_err(|error| Error::database(format!("invalid user email: {error}")))?,
        password_hash: row
            .try_get("password_hash")
            .map_err(|error| Error::database(format!("invalid password hash: {error}")))?,
        created_at: parse_timestamp_column(&row, "created_at")?,
    })
}

fn hydrate_ssh_key(row: sqlx::sqlite::SqliteRow) -> Result<crate::models::SshKey> {
    Ok(crate::models::SshKey {
        id: SshKeyId::from(parse_uuid_column(&row, "id")?),
        user_id: UserId::from(parse_uuid_column(&row, "user_id")?),
        name: row
            .try_get("name")
            .map_err(|error| Error::database(format!("invalid ssh key name: {error}")))?,
        fingerprint: row
            .try_get("fingerprint")
            .map_err(|error| Error::database(format!("invalid ssh key fingerprint: {error}")))?,
        public_key: row
            .try_get("public_key")
            .map_err(|error| Error::database(format!("invalid ssh key material: {error}")))?,
        created_at: parse_timestamp_column(&row, "created_at")?,
    })
}

fn hydrate_pipeline_run(row: sqlx::sqlite::SqliteRow) -> Result<crate::models::PipelineRun> {
    Ok(crate::models::PipelineRun {
        id: PipelineRunId::from(parse_uuid_column(&row, "id")?),
        pipeline_id: PipelineId::from(parse_uuid_column(&row, "pipeline_id")?),
        repo_id: RepoId::from(parse_uuid_column(&row, "repo_id")?),
        status: row
            .try_get("status")
            .map_err(|error| Error::database(format!("invalid pipeline run status: {error}")))?,
        triggered_by: row
            .try_get("triggered_by")
            .map_err(|error| Error::database(format!("invalid pipeline run actor: {error}")))?,
        commit_hash: row
            .try_get("commit_hash")
            .map_err(|error| Error::database(format!("invalid pipeline run commit: {error}")))?,
        started_at: parse_optional_timestamp_column(&row, "started_at")?,
        finished_at: parse_optional_timestamp_column(&row, "finished_at")?,
        created_at: parse_timestamp_column(&row, "created_at")?,
        error: row
            .try_get("error")
            .map_err(|error| Error::database(format!("invalid pipeline run error: {error}")))?,
    })
}

fn hydrate_job(row: sqlx::sqlite::SqliteRow) -> Result<crate::models::Job> {
    let commands: Option<String> = row
        .try_get("commands")
        .map_err(|error| Error::database(format!("invalid job commands column: {error}")))?;
    Ok(crate::models::Job {
        id: JobId::from(parse_uuid_column(&row, "id")?),
        pipeline_run_id: PipelineRunId::from(parse_uuid_column(&row, "pipeline_run_id")?),
        name: row
            .try_get("name")
            .map_err(|error| Error::database(format!("invalid job name: {error}")))?,
        status: row
            .try_get("status")
            .map_err(|error| Error::database(format!("invalid job status: {error}")))?,
        runner_id: row
            .try_get::<Option<String>, _>("runner_id")
            .map_err(|error| Error::database(format!("invalid job runner ID: {error}")))?
            .map(|value| {
                Uuid::parse_str(&value)
                    .map(RunnerId::from)
                    .map_err(|error| Error::database(format!("invalid job runner ID: {error}")))
            })
            .transpose()?,
        started_at: parse_optional_timestamp_column(&row, "started_at")?,
        finished_at: parse_optional_timestamp_column(&row, "finished_at")?,
        retry_count: row
            .try_get("retry_count")
            .map_err(|error| Error::database(format!("invalid job retry count: {error}")))?,
        created_at: parse_timestamp_column(&row, "created_at")?,
        commands: serde_json::from_str(&commands.unwrap_or_else(|| "[]".to_string()))
            .map_err(|error| Error::database(format!("invalid job commands JSON: {error}")))?,
        image: row
            .try_get::<Option<String>, _>("image")
            .map_err(|error| Error::database(format!("invalid job image: {error}")))?
            .unwrap_or_else(|| "rust:latest".to_string()),
        working_dir: row
            .try_get("working_dir")
            .map_err(|error| Error::database(format!("invalid job working directory: {error}")))?,
        timeout_secs: row
            .try_get::<i64, _>("timeout_secs")
            .map_err(|error| Error::database(format!("invalid job timeout: {error}")))?
            .try_into()
            .map_err(|_| Error::database("job timeout cannot be negative"))?,
        result_json: row
            .try_get("result_json")
            .map_err(|error| Error::database(format!("invalid job result: {error}")))?,
        heartbeat_at: parse_optional_timestamp_column(&row, "heartbeat_at")?,
        lease_token: row
            .try_get::<Option<String>, _>("lease_token")
            .map_err(|error| Error::database(format!("invalid job lease token: {error}")))?,
    })
}

fn hydrate_runner(row: sqlx::sqlite::SqliteRow) -> Result<crate::models::Runner> {
    Ok(crate::models::Runner {
        id: RunnerId::from(parse_uuid_column(&row, "id")?),
        name: row
            .try_get("name")
            .map_err(|error| Error::database(format!("invalid runner name: {error}")))?,
        runner_type: row
            .try_get("runner_type")
            .map_err(|error| Error::database(format!("invalid runner type: {error}")))?,
        status: row
            .try_get("status")
            .map_err(|error| Error::database(format!("invalid runner status: {error}")))?,
        last_heartbeat: parse_optional_timestamp_column(&row, "last_heartbeat")?,
        capacity: row
            .try_get("capacity")
            .map_err(|error| Error::database(format!("invalid runner capacity: {error}")))?,
        created_at: parse_timestamp_column(&row, "created_at")?,
    })
}

fn hydrate_event(row: sqlx::sqlite::SqliteRow) -> Result<crate::models::Event> {
    let payload: String = row
        .try_get("payload")
        .map_err(|error| Error::database(format!("invalid event payload column: {error}")))?;
    Ok(crate::models::Event {
        id: parse_uuid_column(&row, "id")?,
        event_type: row
            .try_get("event_type")
            .map_err(|error| Error::database(format!("invalid event type: {error}")))?,
        payload: serde_json::from_str(&payload)
            .map_err(|error| Error::database(format!("invalid event payload JSON: {error}")))?,
        created_at: parse_timestamp_column(&row, "created_at")?,
    })
}

// ============================================================================
// Repository Queries
// ============================================================================

pub struct RepoQueries;

impl RepoQueries {
    /// Create a new repository
    pub async fn create(pool: &Pool, repo: &crate::models::Repository) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO repositories (id, name, owner_id, visibility, git_path, required_checks, deny_non_fast_forward, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(repo.id.to_string())
        .bind(&repo.name)
        .bind(repo.owner_id.to_string())
        .bind(&repo.visibility)
        .bind(&repo.git_path)
        .bind(serde_json::to_string(&repo.required_checks).map_err(|e| {
            Error::database(format!("failed to serialize required checks: {e}"))
        })?)
        .bind(i64::from(repo.deny_non_fast_forward))
        .bind(repo.created_at.to_rfc3339())
        .bind(repo.updated_at.to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create repository: {e}")))?;
        Ok(())
    }

    /// Persist the ref-update policy (#240) for a repository.
    ///
    /// Single conditional UPDATE in an immediate transaction so a policy read
    /// on the push path can never observe a half-applied pair of settings.
    pub async fn update_policy(
        pool: &Pool,
        repo_id: RepoId,
        required_checks: &[String],
        deny_non_fast_forward: bool,
    ) -> Result<()> {
        let required_checks_json = serde_json::to_string(required_checks)
            .map_err(|e| Error::database(format!("failed to serialize required checks: {e}")))?;
        let mut tx = begin_immediate(pool.pool(), "policy update").await?;
        let result = sqlx::query(
            "UPDATE repositories SET required_checks = ?, deny_non_fast_forward = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(&required_checks_json)
        .bind(i64::from(deny_non_fast_forward))
        .bind(Utc::now().to_rfc3339())
        .bind(repo_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::database(format!("failed to update repository policy: {e}")))?;
        if result.rows_affected() == 0 {
            return Err(Error::not_found("repository", repo_id.to_string()));
        }
        tx.commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit policy update: {e}")))?;
        Ok(())
    }

    /// Get a repository by ID
    pub async fn get(pool: &Pool, id: RepoId) -> Result<Option<crate::models::Repository>> {
        let row = sqlx::query("SELECT * FROM repositories WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get repository: {e}")))?;

        match row {
            Some(row) => hydrate_repository(row).map(Some),
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
            .map_err(|e| Error::database(format!("failed to list repositories: {e}")))?;

        let repos = rows
            .into_iter()
            .map(hydrate_repository)
            .collect::<Result<Vec<_>>>()?;

        Ok(repos)
    }

    /// Delete a repository
    pub async fn delete(pool: &Pool, id: RepoId) -> Result<()> {
        // Repository deletion is an explicit destructive operation. Remove
        // all dependent execution history in one transaction so a repository
        // with completed or failed CI runs can be deleted just like an empty
        // repository. The schema intentionally keeps these foreign keys
        // restrictive to protect history during ordinary mutations.
        let mut tx = begin_immediate(pool.pool(), "repository delete").await?;
        let repo_id = id.to_string();
        for statement in [
            "DELETE FROM artifacts WHERE job_id IN (SELECT id FROM jobs WHERE pipeline_run_id IN (SELECT id FROM pipeline_runs WHERE repo_id = ?))",
            "DELETE FROM job_log_chunks WHERE job_id IN (SELECT id FROM jobs WHERE pipeline_run_id IN (SELECT id FROM pipeline_runs WHERE repo_id = ?))",
            "DELETE FROM jobs WHERE pipeline_run_id IN (SELECT id FROM pipeline_runs WHERE repo_id = ?)",
            "DELETE FROM pipeline_runs WHERE repo_id = ?",
            "DELETE FROM pipelines WHERE repo_id = ?",
            "DELETE FROM repositories WHERE id = ?",
        ] {
            sqlx::query(statement)
                .bind(&repo_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| Error::database(format!("failed to delete repository: {e}")))?;
        }
        tx.commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit repository delete: {e}")))?;
        Ok(())
    }

    /// List all repositories
    pub async fn list(pool: &Pool) -> Result<Vec<crate::models::Repository>> {
        let rows = sqlx::query("SELECT * FROM repositories ORDER BY created_at DESC")
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list repositories: {e}")))?;

        let repos = rows
            .into_iter()
            .map(hydrate_repository)
            .collect::<Result<Vec<_>>>()?;

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
        .map_err(|e| Error::database(format!("failed to get repository by owner and name: {e}")))?;

        match row {
            Some(row) => hydrate_repository(row).map(Some),
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
            .map_err(|e| Error::database(format!("failed to get user role: {e}")))?;
        Ok(row.map(|row| row.get::<String, _>("role")))
    }

    /// Set a user's persisted least-privilege role.
    pub async fn set_role(pool: &Pool, id: UserId, role: &str) -> Result<bool> {
        let result = sqlx::query("UPDATE users SET role = ? WHERE id = ?")
            .bind(role)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to set user role: {e}")))?;
        Ok(result.rows_affected() == 1)
    }

    /// Count users currently holding a persisted role.
    pub async fn count_role(pool: &Pool, role: &str) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM users WHERE role = ?")
            .bind(role)
            .fetch_one(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to count user roles: {e}")))?;
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
        .map_err(|e| Error::database(format!("failed to create user: {e}")))?;
        Ok(())
    }

    /// Create a user with an explicitly selected least-privilege role.
    ///
    /// This is intentionally separate from `create` so existing callers retain
    /// the schema default while administrative bootstrap can assign `admin`
    /// atomically with the account insert.
    pub async fn create_with_role(
        pool: &Pool,
        user: &crate::models::User,
        role: &str,
    ) -> Result<()> {
        if !matches!(role, "admin" | "maintainer" | "developer" | "read_only") {
            return Err(Error::invalid_input(format!(
                "unsupported user role: {role}"
            )));
        }
        sqlx::query(
            r#"
            INSERT INTO users (id, username, email, password_hash, role, created_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(user.id.to_string())
        .bind(&user.username)
        .bind(&user.email)
        .bind(&user.password_hash)
        .bind(role)
        .bind(user.created_at.to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create user with role: {e}")))?;
        Ok(())
    }

    /// Get a user by ID
    pub async fn get(pool: &Pool, id: UserId) -> Result<Option<crate::models::User>> {
        let row = sqlx::query("SELECT * FROM users WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get user: {e}")))?;

        row.map(hydrate_user).transpose()
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
            .map_err(|e| Error::database(format!("failed to get user by username: {e}")))?;

        row.map(hydrate_user).transpose()
    }

    /// List all users
    pub async fn list(pool: &Pool) -> Result<Vec<crate::models::User>> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY created_at DESC")
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list users: {e}")))?;

        let users = rows
            .into_iter()
            .map(hydrate_user)
            .collect::<Result<Vec<_>>>()?;

        Ok(users)
    }
}

// ============================================================================
// SSH Key Queries
// ============================================================================

pub struct SshKeyQueries;

impl SshKeyQueries {
    /// Register a new SSH public key for a user. Duplicate fingerprints are
    /// rejected: one public key maps to exactly one account.
    pub async fn create(pool: &Pool, key: &crate::models::SshKey) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO ssh_keys (id, user_id, name, fingerprint, public_key, created_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(key.id.to_string())
        .bind(key.user_id.to_string())
        .bind(&key.name)
        .bind(&key.fingerprint)
        .bind(&key.public_key)
        .bind(key.created_at.to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint failed") {
                Error::invalid_input(format!(
                    "ssh key fingerprint {} is already registered",
                    key.fingerprint
                ))
            } else {
                Error::database(format!("failed to create ssh key: {e}"))
            }
        })?;
        Ok(())
    }

    /// Get a key record by id.
    pub async fn get(pool: &Pool, id: SshKeyId) -> Result<Option<crate::models::SshKey>> {
        let row = sqlx::query("SELECT * FROM ssh_keys WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get ssh key: {e}")))?;

        row.map(hydrate_ssh_key).transpose()
    }

    /// Look up a key by its OpenSSH fingerprint — the identity the git
    /// transport's public-key authentication resolves against.
    pub async fn find_by_fingerprint(
        pool: &Pool,
        fingerprint: &str,
    ) -> Result<Option<crate::models::SshKey>> {
        let row = sqlx::query("SELECT * FROM ssh_keys WHERE fingerprint = ?")
            .bind(fingerprint)
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| {
                Error::database(format!("failed to look up ssh key by fingerprint: {e}"))
            })?;

        row.map(hydrate_ssh_key).transpose()
    }

    /// List every key registered to a user, newest first.
    pub async fn list_by_user(pool: &Pool, user_id: UserId) -> Result<Vec<crate::models::SshKey>> {
        let rows = sqlx::query("SELECT * FROM ssh_keys WHERE user_id = ? ORDER BY created_at DESC")
            .bind(user_id.to_string())
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list ssh keys: {e}")))?;

        rows.into_iter().map(hydrate_ssh_key).collect()
    }

    /// Delete a key by id, but only when it belongs to `user_id`. Returns
    /// whether a row was removed.
    pub async fn delete_owned(pool: &Pool, id: SshKeyId, user_id: UserId) -> Result<bool> {
        let result = sqlx::query("DELETE FROM ssh_keys WHERE id = ? AND user_id = ?")
            .bind(id.to_string())
            .bind(user_id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to delete ssh key: {e}")))?;
        Ok(result.rows_affected() == 1)
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
        .map_err(|e| Error::database(format!("failed to create pipeline: {e}")))?;
        Ok(())
    }

    /// Retire the currently active pipeline version for (repo_id, name).
    ///
    /// The partial UNIQUE index `idx_pipelines_active_repo_name` admits only
    /// one active row per repository and name, so a newly pushed version
    /// must deactivate its predecessor or every push after the first fails
    /// run creation with a constraint violation. Superseded rows stay as
    /// history with active = 0.
    pub async fn deactivate_active(pool: &Pool, repo_id: RepoId, name: &str) -> Result<()> {
        sqlx::query(
            "UPDATE pipelines SET active = 0 WHERE repo_id = ? AND name = ? AND active = 1",
        )
        .bind(repo_id.to_string())
        .bind(name)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to deactivate pipeline: {e}")))?;
        Ok(())
    }

    /// Record a push's pipeline version and its new run as one durable step.
    ///
    /// The trigger consumer used to issue three independent autocommit
    /// writes (deactivate predecessor, insert pipeline, insert run): a busy
    /// failure between them left a retired predecessor with no successor or
    /// a pipeline with no run, and a retried trigger then minted a second
    /// pipeline version. One BEGIN IMMEDIATE transaction makes the trio
    /// all-or-nothing so a durable trigger request can never point at a
    /// half-created run.
    pub async fn activate_pipeline_and_create_run(
        pool: &Pool,
        pipeline: &crate::models::Pipeline,
        run: &crate::models::PipelineRun,
    ) -> Result<()> {
        let mut transaction = begin_immediate(pool.pool(), "pipeline activation").await?;
        sqlx::query(
            "UPDATE pipelines SET active = 0 WHERE repo_id = ? AND name = ? AND active = 1",
        )
        .bind(pipeline.repo_id.to_string())
        .bind(&pipeline.name)
        .execute(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to deactivate pipeline: {e}")))?;
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
        .execute(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to create pipeline: {e}")))?;
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
        .execute(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to create pipeline run: {e}")))?;
        transaction
            .commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit pipeline activation: {e}")))?;
        Ok(())
    }

    /// Number of active pipeline versions for (repo_id, name) — at most one
    /// by the partial UNIQUE index; lets callers verify deactivation.
    pub async fn count_active(pool: &Pool, repo_id: RepoId, name: &str) -> Result<i64> {
        let (count,) = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM pipelines WHERE repo_id = ? AND name = ? AND active = 1",
        )
        .bind(repo_id.to_string())
        .bind(name)
        .fetch_one(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to count active pipelines: {e}")))?;
        Ok(count)
    }

    /// Delete a pipeline row outright.
    ///
    /// Only safe for definitions with no runs: runs reference their
    /// pipeline by id, so removing a run-bearing row would orphan that
    /// history. Callers decide (see the API delete route).
    pub async fn delete(pool: &Pool, id: PipelineId) -> Result<bool> {
        let result = sqlx::query("DELETE FROM pipelines WHERE id = ?")
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to delete pipeline: {e}")))?;
        Ok(result.rows_affected() == 1)
    }

    /// Get a pipeline by ID
    pub async fn get(pool: &Pool, id: PipelineId) -> Result<Option<crate::models::Pipeline>> {
        let row = sqlx::query("SELECT * FROM pipelines WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get pipeline: {e}")))?;

        match row {
            Some(row) => hydrate_pipeline(row).map(Some),
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
                .map_err(|e| Error::database(format!("failed to list pipelines: {e}")))?;

        let pipelines = rows
            .into_iter()
            .map(hydrate_pipeline)
            .collect::<Result<Vec<_>>>()?;

        Ok(pipelines)
    }

    /// List the active pipelines, one per (repository, name).
    ///
    /// Retired versions remain as history and are addressable by id, but
    /// they are definitions, not endpoints: returning them here made
    /// `GET /api/pipelines` scan every superseded row ever recorded.
    pub async fn list(pool: &Pool) -> Result<Vec<crate::models::Pipeline>> {
        let rows = sqlx::query("SELECT * FROM pipelines WHERE active = 1 ORDER BY created_at DESC")
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list pipelines: {e}")))?;

        let pipelines = rows
            .into_iter()
            .map(hydrate_pipeline)
            .collect::<Result<Vec<_>>>()?;

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
            INSERT INTO pipeline_runs (id, pipeline_id, repo_id, status, triggered_by, commit_hash, started_at, finished_at, created_at, error)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(&run.error)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create pipeline run: {e}")))?;
        Ok(())
    }

    /// Get a pipeline run by ID
    pub async fn get(pool: &Pool, id: PipelineRunId) -> Result<Option<crate::models::PipelineRun>> {
        let row = sqlx::query("SELECT * FROM pipeline_runs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get pipeline run: {e}")))?;

        match row {
            Some(row) => hydrate_pipeline_run(row).map(Some),
            None => Ok(None),
        }
    }

    /// Update pipeline run status.
    ///
    /// A terminal verdict is final: once a run is `succeeded`, `failed`,
    /// `cancelled`, or timed out, later graders cannot rewrite it (F24 — a
    /// cancelled run whose late head job still executed was resurrected to
    /// `succeeded` here, with honest work but dishonest bookkeeping). The
    /// guard is a single conditional UPDATE so two graders racing cannot
    /// both pass it; a blocked write is a no-op that logs both verdicts.
    /// Re-writing the same terminal status stays allowed (idempotent
    /// re-finalize after a restart).
    pub async fn update_status(pool: &Pool, id: PipelineRunId, status: &str) -> Result<()> {
        Self::update_status_with_error(pool, id, status, None).await
    }

    /// Update pipeline run status, optionally recording the cause of a
    /// non-success verdict.
    ///
    /// Same terminal-verdict guard as [`Self::update_status`]. A supplied
    /// error lands on the run row so a failed run explains itself in the API
    /// without log access; a reason-less rewrite of the same status never
    /// erases a previously recorded reason (first cause wins).
    pub async fn update_status_with_error(
        pool: &Pool,
        id: PipelineRunId,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let finished_at = matches!(
            status,
            "succeeded" | "failed" | "cancelled" | "timed_out" | "timeout" | "timed-out"
        )
        .then(|| Utc::now().to_rfc3339());
        let result = sqlx::query(
            "UPDATE pipeline_runs SET status = ?, finished_at = COALESCE(finished_at, ?), \
             error = COALESCE(?, error) \
             WHERE id = ? AND (status IS NULL OR status NOT IN \
             ('succeeded','failed','cancelled','timed_out','timeout','timed-out') OR status = ?)",
        )
        .bind(status)
        .bind(finished_at.clone())
        .bind(error)
        .bind(id.to_string())
        .bind(status)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to update pipeline run status: {e}")))?;
        if result.rows_affected() == 0 {
            let durable =
                sqlx::query_scalar::<_, String>("SELECT status FROM pipeline_runs WHERE id = ?")
                    .bind(id.to_string())
                    .fetch_optional(pool.pool())
                    .await
                    .map_err(|e| {
                        Error::database(format!("failed to read pipeline run status: {e}"))
                    })?;
            if let Some(durable) = durable {
                if durable != status {
                    tracing::warn!(
                        run = %id,
                        durable = %durable,
                        attempted = %status,
                        "refusing to rewrite a terminal run verdict (F24)"
                    );
                }
            }
        }
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
        .map_err(|e| Error::database(format!("failed to list pipeline runs: {e}")))?;

        let runs = rows
            .into_iter()
            .map(hydrate_pipeline_run)
            .collect::<Result<Vec<_>>>()?;

        Ok(runs)
    }

    /// List all pipeline runs
    pub async fn list(pool: &Pool) -> Result<Vec<crate::models::PipelineRun>> {
        let rows = sqlx::query("SELECT * FROM pipeline_runs ORDER BY created_at DESC")
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list pipeline runs: {e}")))?;

        let runs = rows
            .into_iter()
            .map(hydrate_pipeline_run)
            .collect::<Result<Vec<_>>>()?;

        Ok(runs)
    }

    /// List per-pipeline outcomes recorded for one commit, latest first (#240).
    ///
    /// The ref-update policy matches on the exact commit hash a push wants to
    /// install, so no ancestry walk is needed — the receive path only asks
    /// "has this commit already built green". Hash comparison is
    /// case-insensitive because git clients send lowercase hex while older
    /// rows may carry uppercase from external triggers.
    pub async fn list_commit_statuses(
        pool: &Pool,
        repo_id: RepoId,
        commit_hash: &str,
    ) -> Result<Vec<CommitRunStatus>> {
        let rows = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT p.name, r.status FROM pipeline_runs r \
             JOIN pipelines p ON r.pipeline_id = p.id \
             WHERE r.repo_id = ? AND LOWER(r.commit_hash) = LOWER(?) \
             ORDER BY r.created_at DESC",
        )
        .bind(repo_id.to_string())
        .bind(commit_hash)
        .fetch_all(pool.pool())
        .await
        .map_err(|e| {
            Error::database(format!(
                "failed to list commit statuses for ref policy: {e}"
            ))
        })?;

        Ok(rows
            .into_iter()
            .map(|(pipeline, status)| CommitRunStatus { pipeline, status })
            .collect())
    }
}

/// A single pipeline run's outcome for one commit, as read from the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRunStatus {
    pub pipeline: String,
    pub status: Option<String>,
}

/// The outcome of one required check for a commit under the ref-update policy.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequiredCheckStatus {
    /// Pipeline name the check gates on.
    pub check: String,
    /// Latest durable run status for that pipeline on the commit; `None`
    /// when the pipeline has never run on it.
    pub status: Option<String>,
}

impl RequiredCheckStatus {
    /// Green means the pipeline's latest run for the commit succeeded. A
    /// pending, running, cancelled, failed, or never-run check is not green.
    pub fn is_green(&self) -> bool {
        self.status.as_deref() == Some("succeeded")
    }
}

/// True when every required check is green. An empty check list is vacuously
/// satisfied — repositories without a configured policy never block pushes.
pub fn required_checks_satisfied(checks: &[RequiredCheckStatus]) -> bool {
    checks.iter().all(RequiredCheckStatus::is_green)
}

/// Aggregate raw per-run statuses into one entry per required check.
///
/// The most recent run of a pipeline wins; `run_statuses` must be ordered
/// latest-first (as `PipelineRunQueries::list_commit_statuses` returns them),
/// and later duplicate rows for the same pipeline are ignored.
pub fn evaluate_required_checks(
    required_checks: &[String],
    run_statuses: &[CommitRunStatus],
) -> Vec<RequiredCheckStatus> {
    required_checks
        .iter()
        .map(|check| {
            let status = run_statuses
                .iter()
                .find(|run| run.pipeline == *check)
                .and_then(|run| run.status.clone());
            RequiredCheckStatus {
                check: check.clone(),
                status,
            }
        })
        .collect()
}

// ============================================================================
// Job Queries
// ============================================================================

pub struct JobQueries;

/// Maximum size of one runner log append.
pub const MAX_JOB_LOG_CHUNK_BYTES: usize = 64 * 1024;
/// Maximum durable log volume retained for one job.
pub const MAX_JOB_LOG_BYTES: i64 = 16 * 1024 * 1024;

fn is_retryable_log_append_error(error: &Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("database is locked")
        || message.contains("database table is locked")
        || message.contains("database is deadlocked")
        || message.contains("unique constraint failed: job_log_chunks")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct JobLogChunk {
    pub sequence: i64,
    pub chunk: String,
    pub created_at: String,
}

impl JobQueries {
    /// Append a log chunk only when the runner still owns the active lease.
    /// `None` means the job is missing, terminal, or fenced by another lease.
    pub async fn append_log_with_lease(
        pool: &Pool,
        id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
        chunk: &str,
    ) -> Result<Option<i64>> {
        // Runner output can arrive concurrently for stdout and stderr. SQLite
        // transactions are deferred by default, so concurrent writers can
        // both observe the same MAX(sequence) and one loses on the composite
        // primary key. Retry only the two transient write races; lease and
        // validation errors must remain immediate and observable.
        const MAX_APPEND_ATTEMPTS: usize = 5;
        for attempt in 0..MAX_APPEND_ATTEMPTS {
            match Self::append_log_with_lease_once(pool, id, runner_id, lease_token, chunk).await {
                Ok(result) => return Ok(result),
                Err(error)
                    if attempt + 1 < MAX_APPEND_ATTEMPTS
                        && is_retryable_log_append_error(&error) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5 * (attempt as u64 + 1)))
                        .await;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("log append retry loop must return")
    }

    async fn append_log_with_lease_once(
        pool: &Pool,
        id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
        chunk: &str,
    ) -> Result<Option<i64>> {
        if chunk.is_empty() {
            return Ok(None);
        }
        if chunk.len() > MAX_JOB_LOG_CHUNK_BYTES {
            return Err(Error::invalid_input(format!(
                "job log chunk exceeds {MAX_JOB_LOG_CHUNK_BYTES} bytes"
            )));
        }

        // Take SQLite's write lock before the authorization and sequence
        // reads. A deferred transaction lets concurrent appenders all read
        // the same MAX(sequence), then contend while upgrading to a writer;
        // SQLite can report that upgrade as `database is deadlocked` even
        // with a busy timeout. BEGIN IMMEDIATE serializes only this short
        // append transaction and keeps the lease check plus sequence
        // allocation atomic.
        let mut conn = pool.pool().acquire().await.map_err(|e| {
            Error::database(format!("failed to acquire log append connection: {e}"))
        })?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::database(format!("failed to begin log append: {e}")))?;
        let Some(job) = sqlx::query("SELECT runner_id, lease_token, status FROM jobs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| Error::database(format!("failed to authorize log append: {e}")))?
        else {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Ok(None);
        };

        let assigned_runner: Option<String> = job.get("runner_id");
        let current_token: Option<String> = job.get("lease_token");
        let status: String = job.get("status");
        if assigned_runner.as_deref() != Some(&runner_id.to_string())
            || current_token.as_deref() != Some(lease_token)
            || !matches!(status.as_str(), "assigned" | "running")
        {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Ok(None);
        }

        let total: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(length(chunk)), 0) FROM job_log_chunks WHERE job_id = ?",
        )
        .bind(id.to_string())
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| Error::database(format!("failed to measure job logs: {e}")))?;
        if total + chunk.len() as i64 > MAX_JOB_LOG_BYTES {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(Error::invalid_input(format!(
                "job logs exceed {MAX_JOB_LOG_BYTES} bytes"
            )));
        }

        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), -1) + 1 FROM job_log_chunks WHERE job_id = ?",
        )
        .bind(id.to_string())
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| Error::database(format!("failed to allocate log sequence: {e}")))?;
        sqlx::query(
            "INSERT INTO job_log_chunks (job_id, sequence, chunk, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(sequence)
        .bind(chunk)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::database(format!("failed to append job log: {e}")))?;
        sqlx::query("COMMIT")
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::database(format!("failed to commit log append: {e}")))?;
        Ok(Some(sequence))
    }

    /// Read durable log chunks in append order.
    pub async fn list_logs(pool: &Pool, id: JobId) -> Result<Vec<JobLogChunk>> {
        let rows = sqlx::query(
            "SELECT sequence, chunk, created_at FROM job_log_chunks WHERE job_id = ? ORDER BY sequence ASC",
        )
        .bind(id.to_string())
        .fetch_all(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to list job logs: {e}")))?;
        Ok(rows
            .into_iter()
            .map(|row| JobLogChunk {
                sequence: row.get("sequence"),
                chunk: row.get("chunk"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    /// Check a runner lease without mutating job state.
    pub async fn lease_is_active(
        pool: &Pool,
        id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
    ) -> Result<bool> {
        let row = sqlx::query(
            "SELECT 1 FROM jobs WHERE id = ? AND runner_id = ? AND lease_token = ? AND status IN ('assigned', 'running')",
        )
        .bind(id.to_string())
        .bind(runner_id.to_string())
        .bind(lease_token)
        .fetch_optional(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to check job lease: {e}")))?;
        Ok(row.is_some())
    }

    /// Record a per-job liveness proof from the executing runner (#243).
    ///
    /// The write is lease-gated exactly like start/complete, so a stale or
    /// superseded token can never refresh anything. A delivered job
    /// heartbeat also refreshes the OWNING runner's `last_heartbeat` and
    /// recovers it from `offline` (never from `busy`): the request arrived
    /// over the same scheduler endpoint as the global heartbeat, so it is
    /// direct evidence the runner is alive and in contact. Under host load
    /// this is what keeps a healthy long build from being fenced while its
    /// runner's coarser global heartbeat happens to starve.
    pub async fn heartbeat(
        pool: &Pool,
        id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
    ) -> Result<bool> {
        let mut transaction = begin_immediate(pool.pool(), "job heartbeat").await?;
        let now = Utc::now().to_rfc3339();
        let job = sqlx::query(
            "UPDATE jobs SET heartbeat_at = ? WHERE id = ? AND runner_id = ? AND lease_token = ? AND status IN ('assigned', 'running')",
        )
        .bind(&now)
        .bind(id.to_string())
        .bind(runner_id.to_string())
        .bind(lease_token)
        .execute(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to record job heartbeat: {e}")))?;
        if job.rows_affected() != 1 {
            // Dropping the transaction rolls it back: nothing to undo.
            return Ok(false);
        }
        sqlx::query(
            "UPDATE runners SET last_heartbeat = ?, status = CASE WHEN status = 'offline' THEN 'online' ELSE status END WHERE id = ?",
        )
        .bind(&now)
        .bind(runner_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to refresh runner heartbeat: {e}")))?;
        transaction
            .commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit job heartbeat: {e}")))?;
        Ok(true)
    }

    /// Persist the scheduler's in-memory lease so durable lease validation
    /// (which reads this row) accepts the lease handed to the runner.
    /// Returns whether a row was updated.
    pub async fn sync_lease(
        pool: &Pool,
        id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE jobs SET runner_id = ?, lease_token = ?, status = 'assigned' WHERE id = ? AND status IN ('queued', 'assigned')",
        )
        .bind(runner_id.to_string())
        .bind(lease_token)
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to sync job lease: {e}")))?;
        Ok(result.rows_affected() > 0)
    }

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
        .map_err(|e| Error::database(format!("failed to get job idempotency key: {e}")))?;
        row.map(|row| {
            let job_id = Uuid::parse_str(&row.get::<String, _>("job_id"))
                .map(JobId::from)
                .map_err(|e| Error::database(format!("invalid stored idempotent job ID: {e}")))?;
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
        .map_err(|e| Error::database(format!("failed to reserve job idempotency key: {e}")))?;
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
            .map_err(|e| Error::database(format!("failed to release job idempotency key: {e}")))?;
        Ok(())
    }

    /// Create a new job
    pub async fn create(pool: &Pool, job: &crate::models::Job) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO jobs (id, pipeline_run_id, name, status, runner_id, started_at, finished_at, retry_count, created_at, commands, image, working_dir, timeout_secs, result_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(&job.image)
        .bind(&job.working_dir)
        .bind(i64::try_from(job.timeout_secs).unwrap_or(i64::MAX))
        .bind(&job.result_json)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create job: {e}")))?;
        Ok(())
    }

    /// Create the job row for an enqueue, or open the queue gate on the row
    /// durable planning already wrote.
    ///
    /// Since durable DAG planning (R6.1), a run's full job set is persisted
    /// as `pending` rows at trigger time; the enqueue that actually releases
    /// a job to a runner must then fill in the execution definition and flip
    /// the status rather than fail on the duplicate id. The conflict update
    /// deliberately touches only the definition columns and the status: the
    /// planned row's name, timestamps, and retry count stay authoritative.
    pub async fn create_or_open_queue(pool: &Pool, job: &crate::models::Job) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO jobs (id, pipeline_run_id, name, status, runner_id, started_at, finished_at, retry_count, created_at, commands, image, working_dir, timeout_secs, result_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                status = 'queued',
                commands = excluded.commands,
                image = excluded.image,
                working_dir = excluded.working_dir,
                timeout_secs = excluded.timeout_secs
            "#,
        )
        .bind(job.id.to_string())
        .bind(job.pipeline_run_id.to_string())
        .bind(&job.name)
        .bind("queued")
        .bind(job.runner_id.map(|id| id.to_string()))
        .bind(job.started_at.map(|dt| dt.to_rfc3339()))
        .bind(job.finished_at.map(|dt| dt.to_rfc3339()))
        .bind(job.retry_count)
        .bind(job.created_at.to_rfc3339())
        .bind(serde_json::to_string(&job.commands).unwrap_or_else(|_| "[]".to_string()))
        .bind(&job.image)
        .bind(&job.working_dir)
        .bind(i64::try_from(job.timeout_secs).unwrap_or(i64::MAX))
        .bind(&job.result_json)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to enqueue job: {e}")))?;
        Ok(())
    }

    /// Get a job by ID
    pub async fn get(pool: &Pool, id: JobId) -> Result<Option<crate::models::Job>> {
        let row = sqlx::query("SELECT * FROM jobs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get job: {e}")))?;

        row.map(hydrate_job).transpose()
    }

    /// Update job status
    pub async fn update_status(pool: &Pool, id: JobId, status: &str) -> Result<()> {
        sqlx::query("UPDATE jobs SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to update job status: {e}")))?;
        Ok(())
    }

    /// Cancel every never-dispatched (`pending`/`queued`) job of one run and
    /// return how many rows changed.
    ///
    /// When a run's lifecycle ends in `failed`/`cancelled`, the rows that
    /// were planned but never dispatched must not outlive it: the run is
    /// terminal, so nothing will ever release their stage, and a zombie
    /// `pending` row both misstates the run's history and keeps `GET
    /// /pipeline-runs/{id}/jobs` showing work that can never run. Only
    /// runner-untouched states are swept, matching `cancel_doomed_rows` —
    /// `assigned`/`running` rows belong to the runner lifecycle.
    pub async fn cancel_unclaimed_for_run(pool: &Pool, run_id: PipelineRunId) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE jobs SET status = ?, finished_at = COALESCE(finished_at, ?) \
             WHERE pipeline_run_id = ? AND status IN ('pending', 'queued')",
        )
        .bind(JobStatus::Cancelled.as_str())
        .bind(Utc::now().to_rfc3339())
        .bind(run_id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to cancel unclaimed jobs: {e}")))?;
        Ok(result.rows_affected())
    }

    /// Cancel every never-dispatched (`pending`/`queued`) job row whose run
    /// is already terminal, across all runs at once.
    ///
    /// A row under a terminal run can never be dispatched again, so it is
    /// inert garbage that keeps job counts wrong forever — the durable
    /// complement to the per-run sweeps performed at finalization time,
    /// catching rows stranded by paths that predate those sweeps (observed
    /// 2026-10-07: 1478 such rows accumulated under failed/cancelled runs).
    /// Only `pending`/`queued` rows are touched: `assigned`/`running` rows
    /// belong to the runner lifecycle and are reaped by the lease and
    /// timeout machinery.
    pub async fn cancel_unclaimed_in_terminal_runs(pool: &Pool) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE jobs SET status = ?, finished_at = COALESCE(finished_at, ?) \
             WHERE status IN ('pending', 'queued') AND pipeline_run_id IN \
             (SELECT id FROM pipeline_runs WHERE status IN \
             ('succeeded', 'failed', 'cancelled', 'timed_out', 'timeout', 'timed-out'))",
        )
        .bind(JobStatus::Cancelled.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to cancel jobs under terminal runs: {e}")))?;
        Ok(result.rows_affected())
    }

    /// Requeue an assigned job and clear its runner fencing token.
    ///
    /// A scheduler may have already persisted `queued` while retaining a
    /// stale runner assignment (for example, after a runner-loss recovery
    /// race). Treat that state as requeueable too; leaving `runner_id` set
    /// prevents the next scheduler tick from assigning the job elsewhere.
    pub async fn requeue(pool: &Pool, id: JobId) -> Result<()> {
        sqlx::query(
            "UPDATE jobs SET status = 'queued', runner_id = NULL, started_at = NULL, lease_token = NULL WHERE id = ? AND status IN ('pending', 'queued', 'assigned', 'running') AND runner_id IS NOT NULL",
        )
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to requeue job: {e}")))?;
        Ok(())
    }

    /// Grade a running job as failed after its runner was lost.
    ///
    /// Runner-loss handling must never requeue a `running` row: the original
    /// sandbox may still be executing, and a second execution of the same job
    /// would race it (duplicate containers, duelling log appends, rejected
    /// completions). Fencing the row as failed matches the recovery contract
    /// of `requeue_inflight` for running rows and lets the pipeline finalize
    /// deterministically; the abandoned-container reconciler collects the
    /// orphaned sandbox after its grace period.
    pub async fn fail_lost(pool: &Pool, id: JobId) -> Result<()> {
        sqlx::query(
            "UPDATE jobs SET status = 'failed', runner_id = NULL, lease_token = NULL, finished_at = ?, result_json = ? WHERE id = ? AND status = 'running'",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(r#"{"status":"failed","reason":"runner_lost_while_running"}"#)
        .bind(id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to fence lost job: {e}")))?;
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
        Self::set_definition_with_image(pool, id, commands, "rust:latest", working_dir).await
    }

    pub async fn set_definition_with_image(
        pool: &Pool,
        id: JobId,
        commands: &[String],
        image: &str,
        working_dir: Option<&str>,
    ) -> Result<()> {
        Self::set_definition_with_image_and_timeout(pool, id, commands, image, working_dir, 300)
            .await
    }

    pub async fn set_definition_with_image_and_timeout(
        pool: &Pool,
        id: JobId,
        commands: &[String],
        image: &str,
        working_dir: Option<&str>,
        timeout_secs: u64,
    ) -> Result<()> {
        let commands_json = serde_json::to_string(commands)
            .map_err(|e| Error::database(format!("failed to encode job commands: {e}")))?;
        let timeout_secs = i64::try_from(timeout_secs)
            .map_err(|_| Error::invalid_input("job timeout exceeds database range"))?;
        sqlx::query(
            "UPDATE jobs SET commands = ?, image = ?, working_dir = ?, timeout_secs = ? WHERE id = ?",
        )
            .bind(commands_json)
            .bind(image)
            .bind(working_dir)
            .bind(timeout_secs)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to persist job definition: {e}")))?;
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
            .map_err(|e| Error::database(format!("failed to persist job receipt: {e}")))?;
        Ok(())
    }

    /// Assign a runner to a job
    pub async fn assign(pool: &Pool, id: JobId, runner_id: RunnerId) -> Result<()> {
        sqlx::query("UPDATE jobs SET runner_id = ?, status = 'assigned' WHERE id = ?")
            .bind(runner_id.to_string())
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to assign job: {e}")))?;
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
        .map_err(|e| Error::database(format!("failed to assign job lease: {e}")))?;
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
        .map_err(|e| Error::database(format!("failed to start job: {e}")))?;
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
        .map_err(|e| Error::database(format!("failed to start job with lease: {e}")))?;
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
        .map_err(|e| Error::database(format!("failed to complete job with lease: {e}")))?;
        Ok(result.rows_affected() == 1)
    }

    /// Complete a leased job and enqueue its external publication atomically.
    /// Both writes share one SQLite transaction, so a publication enqueue
    /// failure cannot leave a terminal job without durable publication work.
    #[allow(clippy::too_many_arguments)]
    pub async fn complete_with_lease_and_publication(
        pool: &Pool,
        id: JobId,
        runner_id: RunnerId,
        lease_token: &str,
        status: &str,
        result_json: &str,
        provider: &str,
        kind: &str,
        payload: &str,
    ) -> Result<bool> {
        if provider.is_empty() || kind.is_empty() || payload.is_empty() {
            return Err(Error::invalid_input("publication fields must not be empty"));
        }
        let mut tx = begin_immediate(pool.pool(), "completion").await?;
        let updated = sqlx::query(
            "UPDATE jobs SET status = ?, finished_at = ?, result_json = ?, lease_token = NULL WHERE id = ? AND runner_id = ? AND lease_token = ? AND status IN ('assigned', 'running')",
        )
        .bind(status)
        .bind(Utc::now().to_rfc3339())
        .bind(result_json)
        .bind(id.to_string())
        .bind(runner_id.to_string())
        .bind(lease_token)
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::database(format!("failed to complete job: {e}")))?;
        if updated.rows_affected() != 1 {
            return Ok(false);
        }
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO publication_outbox (id, job_id, provider, kind, payload, state, attempts, next_attempt_at, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'pending', 0, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(id.to_string())
        .bind(provider)
        .bind(kind)
        .bind(payload)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::database(format!("failed to enqueue publication: {e}")))?;
        let stored: String = sqlx::query_scalar(
            "SELECT payload FROM publication_outbox WHERE job_id = ? AND provider = ? AND kind = ?",
        )
        .bind(id.to_string())
        .bind(provider)
        .bind(kind)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Error::database(format!("failed to verify publication: {e}")))?;
        if stored != payload {
            return Err(Error::invalid_input(
                "publication already exists with a conflicting payload",
            ));
        }
        tx.commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit completion: {e}")))?;
        Ok(true)
    }

    /// Persist an operator cancellation as a terminal job transition.
    ///
    /// Idempotent: a row that already reached a terminal state is left
    /// untouched and reported as success. The single conditional UPDATE
    /// replaces the old read-then-write pair, which could clobber a
    /// concurrently-completed job back into `cancelled` (the check and the
    /// write were two separate statements with no status guard). The write
    /// goes through `persist_with_retry` because it sits on the API's
    /// interactive path — through a write storm it used to 500 with
    /// "Failed to persist cancellation" while the busy timeout expired.
    pub async fn cancel(pool: &Pool, id: JobId, result_json: &str) -> Result<()> {
        let cancelled = persist_with_retry(|| async {
            sqlx::query(
                "UPDATE jobs SET status = 'cancelled', finished_at = ?, result_json = ? \
                 WHERE id = ? AND status IN ('pending', 'queued', 'assigned', 'running')",
            )
            .bind(Utc::now().to_rfc3339())
            .bind(result_json)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to cancel job: {e}")))
        })
        .await?;
        if cancelled.rows_affected() > 0 {
            return Ok(());
        }
        // Nothing was updated: the row is either already terminal
        // (idempotent success — an operator double-click, or the scheduler
        // racing the API to the same verdict) or does not exist.
        match Self::get(pool, id).await? {
            Some(_) => Ok(()),
            None => Err(Error::not_found("job", id)),
        }
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
                .map_err(|e| Error::database(format!("failed to list jobs: {e}")))?;

        let jobs = rows
            .into_iter()
            .map(hydrate_job)
            .collect::<Result<Vec<_>>>()?;

        Ok(jobs)
    }

    /// List jobs that are dispatchable right now: durably `queued` and
    /// waiting for a runner.
    ///
    /// Planned rows (durable DAG planning) sit at `pending` until the engine
    /// releases their dependency stage, so they are deliberately excluded —
    /// scheduler recovery loads this list verbatim, and a planned row that
    /// leaked into it would be dispatched before its predecessors finished.
    pub async fn list_dispatchable(pool: &Pool) -> Result<Vec<crate::models::Job>> {
        let rows =
            sqlx::query("SELECT * FROM jobs WHERE status = 'queued' ORDER BY created_at ASC")
                .fetch_all(pool.pool())
                .await
                .map_err(|e| Error::database(format!("failed to list dispatchable jobs: {e}")))?;

        let jobs = rows
            .into_iter()
            .map(hydrate_job)
            .collect::<Result<Vec<_>>>()?;

        Ok(jobs)
    }

    /// List every job currently recorded as running, with its durable lease
    /// token. This is the restart re-adoption source (#243): rows the restart
    /// fence deliberately left alive must be mirrored back into the scheduler
    /// so their runners can finish reporting against the durable lease.
    pub async fn list_running(pool: &Pool) -> Result<Vec<crate::models::Job>> {
        let rows =
            sqlx::query("SELECT * FROM jobs WHERE status = 'running' ORDER BY created_at ASC")
                .fetch_all(pool.pool())
                .await
                .map_err(|e| Error::database(format!("failed to list running jobs: {e}")))?;

        let jobs = rows
            .into_iter()
            .map(hydrate_job)
            .collect::<Result<Vec<_>>>()?;

        Ok(jobs)
    }

    /// The most recent job starts with their queued→started latency, newest
    /// first — the bounded per-job dispatch-latency sample behind
    /// `/queue/status` (R6.4). Latency is computed from the stored RFC 3339
    /// timestamps; a row whose timestamps fail to parse is skipped rather
    /// than corrupting the sample.
    pub async fn recent_dispatch_latencies(pool: &Pool, limit: i64) -> Result<Vec<(JobId, i64)>> {
        let rows = sqlx::query(
            "SELECT j.id AS job_id, j.started_at AS started_at, j.created_at AS created_at \
             FROM jobs j WHERE j.started_at IS NOT NULL \
             ORDER BY j.started_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to list dispatch latencies: {e}")))?;

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let job_id = parse_uuid_column(&row, "job_id").ok()?;
                let started_at: String = row.try_get("started_at").ok()?;
                let created_at: String = row.try_get("created_at").ok()?;
                let started = DateTime::parse_from_rfc3339(&started_at).ok()?;
                let created = DateTime::parse_from_rfc3339(&created_at).ok()?;
                let latency = (started - created).num_seconds();
                Some((JobId(job_id), latency))
            })
            .collect())
    }

    /// Recover jobs that were in flight when the scheduler stopped. Assigned
    /// jobs have not started execution and are safe to requeue. Running jobs
    /// are fenced as failed instead of being re-run automatically: the old
    /// runner may still be alive, and requeueing would permit duplicate side
    /// effects without a durable runner-generation lease.
    ///
    /// Since #243 the fence is liveness-gated: a running job whose own proof
    /// of life (`heartbeat_at`, falling back to the runner's heartbeat) is
    /// fresher than `fence_grace_secs` survives the restart. The scheduler
    /// re-adopts those rows into its mirror and their runners complete them
    /// against the durable lease; only jobs that were already silent before
    /// the restart are failed here.
    pub async fn requeue_inflight(pool: &Pool, fence_grace_secs: i64) -> Result<u64> {
        let mut transaction = begin_immediate(pool.pool(), "recovery").await?;
        let queued_with_runner = sqlx::query(
            "UPDATE jobs SET runner_id = NULL, started_at = NULL, lease_token = NULL WHERE status = 'queued' AND runner_id IS NOT NULL",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to clear queued runner assignments: {e}")))?;
        let assigned = sqlx::query(
            "UPDATE jobs SET status = 'queued', runner_id = NULL, started_at = NULL, lease_token = NULL WHERE status = 'assigned'",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to requeue assigned jobs: {e}")))?;
        let running = sqlx::query(
            "UPDATE jobs SET status = 'failed', runner_id = NULL, lease_token = NULL, finished_at = ?, result_json = ? WHERE status = 'running' AND id IN (\
                SELECT j.id FROM jobs j LEFT JOIN runners r ON r.id = j.runner_id \
                WHERE j.status = 'running' \
                  AND datetime(COALESCE(j.heartbeat_at, r.last_heartbeat, j.started_at), '+' || ? || ' seconds') <= datetime('now')\
            )",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(r#"{"status":"failed","reason":"scheduler_restart_fenced_running_job"}"#)
        .bind(fence_grace_secs)
        .execute(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to fence running jobs: {e}")))?;
        transaction
            .commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit recovery: {e}")))?;
        Ok(queued_with_runner.rows_affected() + assigned.rows_affected() + running.rows_affected())
    }

    /// Mark running jobs whose persisted deadline has elapsed as timed out.
    /// The status predicate makes this safe against a concurrent completion:
    /// only a still-running job can be reconciled by the watchdog.
    pub async fn reconcile_expired(pool: &Pool) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE jobs SET status = 'timed_out', runner_id = NULL, lease_token = NULL, finished_at = ?, result_json = ? WHERE status = 'running' AND started_at IS NOT NULL AND datetime(started_at, '+' || timeout_secs || ' seconds') <= datetime('now')",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(r#"{"status":"timed_out","reason":"job_timeout_reconciled_by_watchdog"}"#)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to reconcile expired jobs: {e}")))?;
        Ok(result.rows_affected())
    }

    /// Grade non-terminal job rows that already carry terminal evidence.
    ///
    /// The F21/F23 one-shot-write-loss class could strand a row with its
    /// completion receipt (`finished_at` + `result_json`) persisted while the
    /// status/started-at write was lost; the job never turns terminal, so
    /// `finalize_pipeline_if_terminal` skips the run forever (F31's residual,
    /// repaired by hand for bd5c8664). The verdict is read from the row's own
    /// recorded receipt and the evidence columns are never rewritten;
    /// unrecognized or unparseable evidence fails closed to `failed` so a
    /// stranded run always reaches a terminal grade.
    pub async fn reconcile_evidence_rows(pool: &Pool) -> Result<u64> {
        let stranded: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, result_json FROM jobs WHERE status IN ('pending', 'queued', 'assigned', 'running') AND finished_at IS NOT NULL AND result_json IS NOT NULL",
        )
        .fetch_all(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to list evidence-stranded jobs: {e}")))?;
        let mut repaired = 0u64;
        for (job_id, result_json) in stranded {
            let verdict = result_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
                .and_then(|receipt| {
                    receipt
                        .get("status")
                        .and_then(|status| status.as_str())
                        .and_then(JobStatus::from_str)
                })
                .filter(JobStatus::is_terminal)
                .unwrap_or(JobStatus::Failed);
            let result = sqlx::query(
                "UPDATE jobs SET status = ?, runner_id = NULL, lease_token = NULL WHERE id = ? AND status IN ('pending', 'queued', 'assigned', 'running') AND finished_at IS NOT NULL",
            )
            .bind(verdict.as_str())
            .bind(&job_id)
            .execute(pool.pool())
            .await
            .map_err(|e| {
                Error::database(format!("failed to grade evidence-stranded job {job_id}: {e}"))
            })?;
            if result.rows_affected() > 0 {
                tracing::warn!(
                    job = %job_id,
                    verdict = verdict.as_str(),
                    "graded evidence-stranded job from its recorded completion receipt"
                );
                repaired += result.rows_affected();
            }
        }
        Ok(repaired)
    }
}

// ============================================================================
// Runner Queries
// ============================================================================

pub struct RunnerQueries;

/// Result of an operator-requested runner retirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerRetirement {
    Retired,
    AlreadyRetired,
    ActiveJobs(i64),
    NotFound,
}

/// Outcome of a name-keyed runner registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerRegistration {
    /// A new registry row was created for this runner.
    Created,
    /// An existing row carrying the same name was adopted and refreshed.
    Refreshed,
}

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
        .map_err(|e| Error::database(format!("failed to create runner: {e}")))?;
        Ok(())
    }

    /// Register a runner by its stable operator-facing name.
    ///
    /// Runner processes are routinely restarted. Registration must therefore
    /// adopt the existing identity instead of inserting a new UUID on every
    /// restart, otherwise the registry accumulates stale capacity. The
    /// unique index on `runners(name)` backs this up under concurrency: the
    /// SELECT-then-INSERT sequence has a race window, and the losing
    /// registration's INSERT trips the index, sending it around the loop
    /// into the refresh path. Exactly one row per name results either way.
    pub async fn register_or_refresh(
        pool: &Pool,
        runner: &crate::models::Runner,
    ) -> Result<(crate::models::Runner, RunnerRegistration)> {
        for _ in 0..3 {
            let existing = sqlx::query(
                "SELECT * FROM runners WHERE name = ? ORDER BY updated_at DESC, created_at DESC LIMIT 1",
            )
                .bind(&runner.name)
                .fetch_optional(pool.pool())
                .await
                .map_err(|error| {
                    Error::database(format!("failed to find runner by name: {error}"))
                })?
                .map(hydrate_runner)
                .transpose()?;

            let Some(mut existing) = existing else {
                match Self::create(pool, runner).await {
                    Ok(()) => return Ok((runner.clone(), RunnerRegistration::Created)),
                    // Lost the race: another registration created the name
                    // first. Loop and refresh that row instead.
                    Err(error) if error.message.contains("UNIQUE constraint failed") => continue,
                    Err(error) => return Err(error),
                }
            };

            sqlx::query(
                "UPDATE runners SET runner_type = ?, status = ?, capacity = ?, labels = ?, last_heartbeat = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&runner.runner_type)
            .bind(&runner.status)
            .bind(runner.capacity)
            .bind("[]")
            .bind(runner.last_heartbeat.map(|date| date.to_rfc3339()))
            .bind(Utc::now().to_rfc3339())
            .bind(existing.id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|error| Error::database(format!("failed to refresh runner: {error}")))?;

            existing.runner_type = runner.runner_type.clone();
            existing.status = runner.status.clone();
            existing.capacity = runner.capacity;
            existing.last_heartbeat = runner.last_heartbeat;
            return Ok((existing, RunnerRegistration::Refreshed));
        }

        Err(Error::database(
            "runner registration raced on the same name repeatedly; giving up",
        ))
    }

    /// Get a runner by ID
    pub async fn get(pool: &Pool, id: RunnerId) -> Result<Option<crate::models::Runner>> {
        let row = sqlx::query("SELECT * FROM runners WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get runner: {e}")))?;

        row.map(hydrate_runner).transpose()
    }

    /// Update runner heartbeat
    pub async fn heartbeat(pool: &Pool, id: RunnerId) -> Result<()> {
        // Durable-write discipline (F21/F23): a heartbeat lost to a transient
        // busy is how a healthy runner gets fenced by the stale sweep while
        // its job is mid-flight (runner_lost, 2026-09-29 incident ledger).
        persist_with_retry(|| async {
            sqlx::query("UPDATE runners SET last_heartbeat = ? WHERE id = ?")
                .bind(Utc::now().to_rfc3339())
                .bind(id.to_string())
                .execute(pool.pool())
                .await
                .map_err(|e| Error::database(format!("failed to update heartbeat: {e}")))
        })
        .await?;
        Ok(())
    }

    /// Update runner status
    pub async fn update_status(pool: &Pool, id: RunnerId, status: &str) -> Result<()> {
        sqlx::query("UPDATE runners SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to update runner status: {e}")))?;
        Ok(())
    }

    /// Mark every `online` runner whose last activity (heartbeat, or
    /// registration when no heartbeat ever arrived) is older than
    /// `timeout_secs` as `offline`, and return how many rows changed.
    ///
    /// This is the durable counterpart of the scheduler's in-memory sweep:
    /// a row registered by a process that has since restarted is invisible
    /// to the memory sweep forever, and its stale `online` status is
    /// exactly the "online status lies" defect (F3/F26 — observed live as a
    /// legacy row kept `online` for two days past its last heartbeat).
    pub async fn mark_stale_offline(pool: &Pool, timeout_secs: i64) -> Result<u64> {
        // Stored timestamps are RFC3339 TEXT: normalize through datetime()
        // before comparing, or same-day rows sort wrong ('T' > ' ' makes a
        // 10-minute-old heartbeat look newer than 90 seconds ago).
        let result = sqlx::query(
            "UPDATE runners SET status = 'offline' \
             WHERE status = 'online' \
               AND datetime(COALESCE(last_heartbeat, created_at)) < \
                   datetime('now', '-' || ? || ' seconds')",
        )
        .bind(timeout_secs)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to mark stale runners offline: {e}")))?;
        Ok(result.rows_affected())
    }

    /// Retire a runner without removing its audit record.
    ///
    /// Retirement is refused while the runner owns an assigned or running
    /// job. The check and status transition share one transaction so an
    /// operator cannot accidentally hide a live worker between the two
    /// operations. Retired runners are already excluded by scheduler
    /// policies that select only `online` runners.
    pub async fn retire_if_idle(pool: &Pool, id: RunnerId) -> Result<RunnerRetirement> {
        let mut transaction = begin_immediate(pool.pool(), "runner retirement").await?;

        let status: Option<String> = sqlx::query_scalar("SELECT status FROM runners WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|e| Error::database(format!("failed to load runner for retirement: {e}")))?;

        let Some(status) = status else {
            transaction.rollback().await.ok();
            return Ok(RunnerRetirement::NotFound);
        };
        if status == "retired" {
            transaction.rollback().await.ok();
            return Ok(RunnerRetirement::AlreadyRetired);
        }

        let active_jobs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE runner_id = ? AND status IN ('assigned', 'running')",
        )
        .bind(id.to_string())
        .fetch_one(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to inspect runner jobs: {e}")))?;

        if active_jobs > 0 {
            transaction.rollback().await.ok();
            return Ok(RunnerRetirement::ActiveJobs(active_jobs));
        }

        sqlx::query("UPDATE runners SET status = 'retired', updated_at = ? WHERE id = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(|e| Error::database(format!("failed to retire runner: {e}")))?;

        transaction
            .commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit runner retirement: {e}")))?;
        Ok(RunnerRetirement::Retired)
    }

    /// List all runners
    pub async fn list(pool: &Pool) -> Result<Vec<crate::models::Runner>> {
        let rows = sqlx::query("SELECT * FROM runners ORDER BY created_at DESC")
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list runners: {e}")))?;

        let runners = rows
            .into_iter()
            .map(hydrate_runner)
            .collect::<Result<Vec<_>>>()?;

        Ok(runners)
    }

    /// List online runners
    pub async fn list_online(pool: &Pool) -> Result<Vec<crate::models::Runner>> {
        let rows =
            sqlx::query("SELECT * FROM runners WHERE status = 'online' ORDER BY created_at DESC")
                .fetch_all(pool.pool())
                .await
                .map_err(|e| Error::database(format!("failed to list online runners: {e}")))?;

        let runners = rows
            .into_iter()
            .map(hydrate_runner)
            .collect::<Result<Vec<_>>>()?;

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
        .map_err(|e| Error::database(format!("failed to create event: {e}")))?;
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
        .map_err(|e| Error::database(format!("failed to list events: {e}")))?;

        let events = rows
            .into_iter()
            .map(hydrate_event)
            .collect::<Result<Vec<_>>>()?;

        Ok(events)
    }

    /// List recent events
    pub async fn list_recent(pool: &Pool, limit: i64) -> Result<Vec<crate::models::Event>> {
        let rows = sqlx::query("SELECT * FROM events ORDER BY created_at DESC LIMIT ?")
            .bind(limit)
            .fetch_all(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to list events: {e}")))?;

        let events = rows
            .into_iter()
            .map(hydrate_event)
            .collect::<Result<Vec<_>>>()?;

        Ok(events)
    }
}

// ============================================================================
// Review queries (ADR 20260905 code review contract)
// ============================================================================

/// A persisted review run.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRun {
    pub id: Uuid,
    pub repo_id: Option<Uuid>,
    pub base_sha: String,
    pub head_sha: String,
    pub idempotency_key: String,
    pub status: gitforge_review::domain::ReviewRunState,
    pub attempt: i64,
    pub receipt_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A persisted review finding.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewFinding {
    pub id: Uuid,
    pub run_id: Uuid,
    pub source: String,
    pub fingerprint: String,
    pub path: String,
    pub line: Option<i64>,
    pub severity: String,
    pub category: String,
    pub title: String,
    pub message: String,
    pub evidence: Option<String>,
    pub confidence: String,
    pub position_status: gitforge_review::domain::PositionStatus,
    pub disposition: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input for [`ReviewQueries::create_or_get_run`].
#[derive(Debug, Clone)]
pub struct NewReviewRun {
    pub repo_id: Option<Uuid>,
    pub base_sha: String,
    pub head_sha: String,
    pub idempotency_key: String,
    pub attempt: i64,
}

/// Input for [`ReviewQueries::insert_finding`]. The fingerprint is derived
/// from these fields via [`gitforge_review::domain::finding_fingerprint`] (ADR R4).
#[derive(Debug, Clone)]
pub struct NewReviewFinding {
    pub run_id: Uuid,
    pub source: String,
    pub file: String,
    pub line: Option<u32>,
    pub severity: String,
    pub category: String,
    pub title: String,
    pub message: String,
    pub evidence: Option<String>,
    pub confidence: String,
    pub position_status: gitforge_review::domain::PositionStatus,
}

/// Outcome of [`ReviewQueries::create_or_get_run`].
#[derive(Debug, Clone, PartialEq)]
pub enum CreateOrGetReviewRun {
    /// A new run was created for this idempotency key.
    Created(ReviewRun),
    /// The key already existed with the same head SHA; the existing run is
    /// returned unchanged.
    Existing(ReviewRun),
    /// The key already existed against a different head SHA. This is a typed
    /// conflict: idempotency keys must never silently reuse another commit.
    HeadConflict {
        existing: ReviewRun,
        requested_head_sha: String,
    },
}

/// Outcome of [`ReviewQueries::insert_finding`].
#[derive(Debug, Clone, PartialEq)]
pub enum FindingInsertOutcome {
    /// The finding was newly inserted.
    Inserted(ReviewFinding),
    /// A finding with the same `(run_id, fingerprint)` already existed; the
    /// stored row is returned and no write occurred.
    Duplicate(ReviewFinding),
}

fn parse_review_run_state(value: String) -> Result<gitforge_review::domain::ReviewRunState> {
    value
        .parse()
        .map_err(|e: String| Error::database(format!("invalid review run status: {e}")))
}

fn parse_position_status(value: String) -> Result<gitforge_review::domain::PositionStatus> {
    value
        .parse()
        .map_err(|e: String| Error::database(format!("invalid position status: {e}")))
}

fn hydrate_review_run(row: sqlx::sqlite::SqliteRow) -> Result<ReviewRun> {
    Ok(ReviewRun {
        id: parse_uuid_column(&row, "id")?,
        repo_id: row
            .try_get::<Option<String>, _>("repo_id")
            .map_err(|error| Error::database(format!("invalid review run repo_id: {error}")))?
            .map(|value| {
                Uuid::parse_str(&value).map_err(|error| {
                    Error::database(format!("invalid review run repo_id: {error}"))
                })
            })
            .transpose()?,
        base_sha: row
            .try_get("base_sha")
            .map_err(|error| Error::database(format!("invalid review run base SHA: {error}")))?,
        head_sha: row
            .try_get("head_sha")
            .map_err(|error| Error::database(format!("invalid review run head SHA: {error}")))?,
        idempotency_key: row.try_get("idempotency_key").map_err(|error| {
            Error::database(format!("invalid review run idempotency key: {error}"))
        })?,
        status: parse_review_run_state(
            row.try_get("status")
                .map_err(|error| Error::database(format!("invalid review run status: {error}")))?,
        )?,
        attempt: row
            .try_get("attempt")
            .map_err(|error| Error::database(format!("invalid review run attempt: {error}")))?,
        receipt_id: row
            .try_get("receipt_id")
            .map_err(|error| Error::database(format!("invalid review run receipt: {error}")))?,
        created_at: parse_timestamp_column(&row, "created_at")?,
        updated_at: parse_timestamp_column(&row, "updated_at")?,
    })
}

fn hydrate_review_finding(row: sqlx::sqlite::SqliteRow) -> Result<ReviewFinding> {
    Ok(ReviewFinding {
        id: parse_uuid_column(&row, "id")?,
        run_id: parse_uuid_column(&row, "run_id")?,
        source: row
            .try_get("source")
            .map_err(|error| Error::database(format!("invalid finding source: {error}")))?,
        fingerprint: row
            .try_get("fingerprint")
            .map_err(|error| Error::database(format!("invalid finding fingerprint: {error}")))?,
        path: row
            .try_get("path")
            .map_err(|error| Error::database(format!("invalid finding path: {error}")))?,
        line: row
            .try_get("line")
            .map_err(|error| Error::database(format!("invalid finding line: {error}")))?,
        severity: row
            .try_get("severity")
            .map_err(|error| Error::database(format!("invalid finding severity: {error}")))?,
        category: row
            .try_get("category")
            .map_err(|error| Error::database(format!("invalid finding category: {error}")))?,
        title: row
            .try_get("title")
            .map_err(|error| Error::database(format!("invalid finding title: {error}")))?,
        message: row
            .try_get("message")
            .map_err(|error| Error::database(format!("invalid finding message: {error}")))?,
        evidence: row
            .try_get("evidence")
            .map_err(|error| Error::database(format!("invalid finding evidence: {error}")))?,
        confidence: row
            .try_get("confidence")
            .map_err(|error| Error::database(format!("invalid finding confidence: {error}")))?,
        position_status: parse_position_status(row.try_get("position_status").map_err(
            |error| Error::database(format!("invalid finding position status: {error}")),
        )?)?,
        disposition: row
            .try_get("disposition")
            .map_err(|error| Error::database(format!("invalid finding disposition: {error}")))?,
        created_at: parse_timestamp_column(&row, "created_at")?,
        updated_at: parse_timestamp_column(&row, "updated_at")?,
    })
}

pub struct ReviewQueries;

impl ReviewQueries {
    /// Create a review run, or return the existing run for the same
    /// idempotency key. A matching key against a different head SHA yields
    /// [`CreateOrGetReviewRun::HeadConflict`] rather than a silent reuse.
    pub async fn create_or_get_run(
        pool: &Pool,
        new_run: &NewReviewRun,
    ) -> Result<CreateOrGetReviewRun> {
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4();
        let insert = sqlx::query(
            r#"
            INSERT INTO review_runs
                (id, repo_id, base_sha, head_sha, idempotency_key, status, attempt,
                 receipt_id, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, 'pending', ?, NULL, ?, ?)
            "#,
        )
        .bind(id.to_string())
        .bind(new_run.repo_id.map(|r| r.to_string()))
        .bind(&new_run.base_sha)
        .bind(&new_run.head_sha)
        .bind(&new_run.idempotency_key)
        .bind(new_run.attempt)
        .bind(&now)
        .bind(&now)
        .execute(pool.pool())
        .await;

        match insert {
            Ok(_) => {
                let run = Self::get_run(pool, id)
                    .await?
                    .ok_or_else(|| Error::database("review run disappeared after insert"))?;
                Ok(CreateOrGetReviewRun::Created(run))
            }
            Err(error) => {
                let message = error.to_string();
                if !message.contains("UNIQUE constraint failed") {
                    return Err(Error::database(format!(
                        "failed to create review run: {error}"
                    )));
                }
                let existing = Self::get_run_by_idempotency_key(pool, &new_run.idempotency_key)
                    .await?
                    .ok_or_else(|| {
                        Error::database(format!(
                            "idempotency conflict for key {:?} but no existing run found",
                            new_run.idempotency_key
                        ))
                    })?;
                if existing.head_sha == new_run.head_sha {
                    Ok(CreateOrGetReviewRun::Existing(existing))
                } else {
                    Ok(CreateOrGetReviewRun::HeadConflict {
                        existing,
                        requested_head_sha: new_run.head_sha.clone(),
                    })
                }
            }
        }
    }

    /// Read a review run by ID.
    pub async fn get_run(pool: &Pool, id: Uuid) -> Result<Option<ReviewRun>> {
        let row = sqlx::query("SELECT * FROM review_runs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get review run: {e}")))?;
        match row {
            Some(row) => hydrate_review_run(row).map(Some),
            None => Ok(None),
        }
    }

    /// Read a review run by idempotency key.
    pub async fn get_run_by_idempotency_key(
        pool: &Pool,
        idempotency_key: &str,
    ) -> Result<Option<ReviewRun>> {
        let row = sqlx::query("SELECT * FROM review_runs WHERE idempotency_key = ?")
            .bind(idempotency_key)
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get review run by key: {e}")))?;
        match row {
            Some(row) => hydrate_review_run(row).map(Some),
            None => Ok(None),
        }
    }

    /// Conditionally advance a run's lifecycle state (ADR R3). The update
    /// only applies when the current stored state permits the transition, so
    /// terminal runs can never re-enter a non-terminal state and concurrent
    /// writers cannot move a run backward. Returns the updated run, `None`
    /// when the run does not exist, and an error when the transition is
    /// invalid for the current state.
    pub async fn transition_run(
        pool: &Pool,
        id: Uuid,
        next: gitforge_review::domain::ReviewRunState,
    ) -> Result<Option<ReviewRun>> {
        let mut tx = begin_immediate(pool.pool(), "review transition").await?;

        let current = sqlx::query("SELECT status FROM review_runs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| Error::database(format!("failed to read review run status: {e}")))?;

        let current = match current {
            Some(row) => {
                let value: String = row
                    .try_get("status")
                    .map_err(|e| Error::database(format!("missing status column: {e}")))?;
                parse_review_run_state(value)?
            }
            None => {
                tx.rollback().await.map_err(|e| {
                    Error::database(format!("failed to roll back review transition: {e}"))
                })?;
                return Ok(None);
            }
        };

        if !current.can_transition_to(next) {
            tx.rollback().await.map_err(|e| {
                Error::database(format!("failed to roll back review transition: {e}"))
            })?;
            return Err(Error::new(
                gitforge_common::ErrorKind::InvalidInput,
                format!("invalid review run transition: {current} → {next}"),
            ));
        }

        let result = sqlx::query(
            "UPDATE review_runs SET status = ?, updated_at = ? WHERE id = ? AND status = ?",
        )
        .bind(next.to_string())
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .bind(current.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::database(format!("failed to update review run status: {e}")))?;

        if result.rows_affected() != 1 {
            // A concurrent writer moved the row between the read and the
            // guarded update; treat as a failed transition.
            tx.rollback().await.map_err(|e| {
                Error::database(format!("failed to roll back review transition: {e}"))
            })?;
            return Err(Error::new(
                gitforge_common::ErrorKind::InvalidInput,
                format!("review run transition lost a race: {current} → {next}"),
            ));
        }

        tx.commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit review transition: {e}")))?;

        Self::get_run(pool, id).await
    }

    /// Atomically claim the oldest pending review run for a worker.
    ///
    /// The claim is the durable hand-off between the submission control plane
    /// and the review workers. It must be safe when many workers poll the
    /// queue concurrently and must preserve the ADR R3 monotonic lifecycle
    /// (`pending` may only advance forward). Because the existing schema does
    /// not carry a `runner_id`/`lease_token` column on `review_runs`, the
    /// claim is encoded purely in the lifecycle state: the row is selected in
    /// deterministic FIFO order, then conditionally advanced from `pending`
    /// to `running` inside a single guarded UPDATE so a racing worker cannot
    /// silently create a second ownership record for the same row.
    ///
    /// Semantics:
    /// - Only `pending` rows are eligible. Terminal rows (`succeeded`,
    ///   `failed`, `cancelled`) and an already-`running` row stay untouched.
    /// - FIFO ordering uses `(created_at ASC, id ASC)`, matching the schema's
    ///   existing index strategy so the queue drains deterministically.
    /// - The `attempt` counter on the run is incremented as part of the
    ///   guarded update. The schema's `attempt` column is the only retry
    ///   counter available; this preserves the existing "attempt is bumped
    ///   when a worker picks up the run" invariant callers can rely on.
    /// - A repeated call after a successful claim yields `None` until a new
    ///   run is submitted (the queue is empty from the worker's view); no
    ///   silent duplicate ownership can be created.
    /// - A racing claim that loses to a concurrent winner returns `None`
    ///   rather than retrying: the scheduler polls, and the next claim will
    ///   find the now-oldest pending row.
    ///
    /// The claim uses `BEGIN IMMEDIATE` to acquire the write lock at
    /// transaction start. This is what keeps the SELECT-and-UPDATE pair
    /// atomic across pooled connections: without `IMMEDIATE`, two
    /// concurrent transactions can each read the same candidate row
    /// without contention and only race when they try to upgrade to a
    /// write lock at UPDATE time, where SQLite returns `SQLITE_BUSY`
    /// instead of queueing under the configured busy timeout. With
    /// `IMMEDIATE`, claimers serialize at `BEGIN`, so the second claimer
    /// waits for the first to commit and then sees the candidate row
    /// already advanced out of `pending`.
    pub async fn claim_pending(pool: &Pool) -> Result<Option<ReviewRun>> {
        let mut tx = begin_immediate(pool.pool(), "review claim").await?;

        // FIFO candidate selection. Held under the IMMEDIATE write lock so
        // no concurrent claimer can advance the same row before the
        // matching UPDATE below commits.
        let candidate: Option<String> = sqlx::query_scalar(
            r#"
            SELECT id FROM review_runs
            WHERE status = 'pending'
            ORDER BY created_at ASC, id ASC
            LIMIT 1
            "#,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| Error::database(format!("failed to select pending run: {e}")))?;

        let Some(candidate_id) = candidate else {
            tx.rollback().await.map_err(|e| {
                Error::database(format!("failed to roll back empty review claim: {e}"))
            })?;
            return Ok(None);
        };

        // Conditional advance inside the same transaction. The
        // `status = 'pending'` guard is what makes the claim
        // double-claim-safe: if a parallel claimer had already won (which
        // cannot happen under BEGIN IMMEDIATE serialization, but is the
        // correct invariant to assert regardless), this UPDATE matches
        // zero rows and we surface `None` to the caller.
        let updated = sqlx::query(
            r#"
            UPDATE review_runs
            SET status = 'running',
                attempt = attempt + 1,
                updated_at = ?
            WHERE id = ?
              AND status = 'pending'
            "#,
        )
        .bind(Utc::now().to_rfc3339())
        .bind(&candidate_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::database(format!("failed to claim review run: {e}")))?;

        if updated.rows_affected() != 1 {
            tx.rollback().await.map_err(|e| {
                Error::database(format!("failed to roll back review claim race: {e}"))
            })?;
            return Ok(None);
        }

        let row = sqlx::query("SELECT * FROM review_runs WHERE id = ?")
            .bind(&candidate_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| Error::database(format!("failed to read claimed review run: {e}")))?;

        tx.commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit review claim: {e}")))?;

        hydrate_review_run(row).map(Some)
    }

    /// Insert a finding for a run, idempotently: a retried insertion of the
    /// same content (same ADR R4 fingerprint) returns the already-stored row
    /// instead of failing. The database CHECK constraint enforces the
    /// line-position invariant (ADR R5) independently of this API.
    pub async fn insert_finding(
        pool: &Pool,
        finding: &NewReviewFinding,
    ) -> Result<FindingInsertOutcome> {
        if !finding.position_status.is_line_position() && finding.line.is_some() {
            return Err(Error::invalid_input(format!(
                "finding with position status '{}' must not carry a line",
                finding.position_status
            )));
        }
        let fingerprint = gitforge_review::domain::finding_fingerprint(
            &finding.file,
            finding.line,
            &finding.category,
            &finding.message,
        );
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4();
        let insert = sqlx::query(
            r#"
            INSERT INTO review_findings
                (id, run_id, source, fingerprint, path, line, severity, category,
                 title, message, evidence, confidence, position_status, disposition,
                 created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)
            "#,
        )
        .bind(id.to_string())
        .bind(finding.run_id.to_string())
        .bind(&finding.source)
        .bind(&fingerprint)
        .bind(&finding.file)
        .bind(finding.line.map(i64::from))
        .bind(&finding.severity)
        .bind(&finding.category)
        .bind(&finding.title)
        .bind(&finding.message)
        .bind(&finding.evidence)
        .bind(&finding.confidence)
        .bind(finding.position_status.to_string())
        .bind(&now)
        .bind(&now)
        .execute(pool.pool())
        .await;

        match insert {
            Ok(_) => {
                let stored = Self::get_finding(pool, id)
                    .await?
                    .ok_or_else(|| Error::database("review finding disappeared after insert"))?;
                Ok(FindingInsertOutcome::Inserted(stored))
            }
            Err(error) => {
                let message = error.to_string();
                if message.contains("UNIQUE constraint failed") && message.contains("fingerprint") {
                    let existing =
                        Self::get_finding_by_fingerprint(pool, finding.run_id, &fingerprint)
                            .await?
                            .ok_or_else(|| {
                                Error::database(format!(
                                    "fingerprint conflict for run {} but no existing finding found",
                                    finding.run_id
                                ))
                            })?;
                    Ok(FindingInsertOutcome::Duplicate(existing))
                } else {
                    Err(Error::database(format!(
                        "failed to insert finding: {error}"
                    )))
                }
            }
        }
    }

    /// Read a finding by ID.
    pub async fn get_finding(pool: &Pool, id: Uuid) -> Result<Option<ReviewFinding>> {
        let row = sqlx::query("SELECT * FROM review_findings WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get review finding: {e}")))?;
        match row {
            Some(row) => hydrate_review_finding(row).map(Some),
            None => Ok(None),
        }
    }

    /// Read a finding by its run and fingerprint.
    pub async fn get_finding_by_fingerprint(
        pool: &Pool,
        run_id: Uuid,
        fingerprint: &str,
    ) -> Result<Option<ReviewFinding>> {
        let row = sqlx::query("SELECT * FROM review_findings WHERE run_id = ? AND fingerprint = ?")
            .bind(run_id.to_string())
            .bind(fingerprint)
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get review finding: {e}")))?;
        match row {
            Some(row) => hydrate_review_finding(row).map(Some),
            None => Ok(None),
        }
    }

    /// List all findings for a run, ordered by path then line.
    pub async fn list_findings(pool: &Pool, run_id: Uuid) -> Result<Vec<ReviewFinding>> {
        let rows =
            sqlx::query("SELECT * FROM review_findings WHERE run_id = ? ORDER BY path, line")
                .bind(run_id.to_string())
                .fetch_all(pool.pool())
                .await
                .map_err(|e| Error::database(format!("failed to list review findings: {e}")))?;
        rows.into_iter().map(hydrate_review_finding).collect()
    }
}

// ============================================================================
// Pipeline Trigger Requests
// ============================================================================

pub struct TriggerRequestQueries;

impl TriggerRequestQueries {
    /// Record a trigger request, or return the in-flight request that
    /// already covers (repo_id, new_hash).
    ///
    /// The second element is `true` when this call created the row. The
    /// dedup covers only `pending`/`processing` rows (partial unique index
    /// `idx_trigger_requests_active`), so the webhook and a manual API
    /// trigger racing on the same commit collapse into one run, while an
    /// explicit re-trigger of an old commit after completion still works.
    pub async fn create_or_existing(
        pool: &Pool,
        request: &crate::models::PipelineTriggerRequest,
    ) -> Result<(crate::models::PipelineTriggerRequest, bool)> {
        if let Some(existing) = Self::find_active(pool, request.repo_id, &request.new_hash).await? {
            return Ok((existing, false));
        }
        persist_with_retry(|| async {
            let mut transaction =
                begin_immediate(pool.pool(), "trigger request insert").await?;
            let existing_id: Option<String> =
                sqlx::query_scalar(
                    "SELECT id FROM pipeline_trigger_requests \
                     WHERE repo_id = ? AND new_hash = ? AND status IN ('pending', 'processing') \
                     LIMIT 1",
                )
                .bind(request.repo_id.to_string())
                .bind(&request.new_hash)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(|e| {
                    Error::database(format!("failed to check trigger request dedup: {e}"))
                })?;
            if let Some(existing_id) = existing_id {
                // Nothing was written; end the transaction and surface the
                // row that already covers this commit.
                transaction
                    .commit()
                    .await
                    .map_err(|e| Error::database(format!("failed to commit dedup check: {e}")))?;
                let existing = Self::get(pool, Uuid::parse_str(&existing_id).map_err(|e| {
                    Error::database(format!("invalid trigger request id: {e}"))
                })?)
                .await?
                .ok_or_else(|| {
                    Error::database("trigger request vanished between check and read".to_string())
                })?;
                return Ok((existing, false));
            }
            sqlx::query(
                r#"
                INSERT INTO pipeline_trigger_requests
                    (id, repo_id, ref_name, old_hash, new_hash, status, run_id, attempts, error, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(request.id.to_string())
            .bind(request.repo_id.to_string())
            .bind(&request.ref_name)
            .bind(&request.old_hash)
            .bind(&request.new_hash)
            .bind(&request.status)
            .bind(request.run_id.map(|id| id.to_string()))
            .bind(request.attempts)
            .bind(&request.error)
            .bind(request.created_at.to_rfc3339())
            .bind(request.updated_at.to_rfc3339())
            .execute(&mut *transaction)
            .await
            .map_err(|e| Error::database(format!("failed to insert trigger request: {e}")))?;
            transaction
                .commit()
                .await
                .map_err(|e| Error::database(format!("failed to commit trigger request: {e}")))?;
            Ok((request.clone(), true))
        })
        .await
    }

    /// The in-flight (`pending`/`processing`) request covering (repo, commit),
    /// if any.
    pub async fn find_active(
        pool: &Pool,
        repo_id: RepoId,
        new_hash: &str,
    ) -> Result<Option<crate::models::PipelineTriggerRequest>> {
        let id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM pipeline_trigger_requests \
             WHERE repo_id = ? AND new_hash = ? AND status IN ('pending', 'processing') \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(repo_id.to_string())
        .bind(new_hash)
        .fetch_optional(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to find active trigger request: {e}")))?;
        match id {
            Some(id) => {
                let uuid = Uuid::parse_str(&id)
                    .map_err(|e| Error::database(format!("invalid trigger request id: {e}")))?;
                Self::get(pool, uuid).await
            }
            None => Ok(None),
        }
    }

    /// Fetch one trigger request by id.
    pub async fn get(
        pool: &Pool,
        id: Uuid,
    ) -> Result<Option<crate::models::PipelineTriggerRequest>> {
        let row = sqlx::query("SELECT * FROM pipeline_trigger_requests WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to get trigger request: {e}")))?;
        row.map(hydrate_trigger_request).transpose()
    }

    /// Mark a request as being processed by the consumer and count the
    /// attempt. Accepts both `pending` (first pickup, retries) and
    /// `processing` (a redelivery while a previous pass is still counted),
    /// so an at-least-once redelivery never dead-ends on a state mismatch.
    pub async fn mark_processing(pool: &Pool, id: Uuid) -> Result<()> {
        persist_with_retry(|| async {
            sqlx::query(
                "UPDATE pipeline_trigger_requests \
                 SET status = 'processing', attempts = attempts + 1, updated_at = ? \
                 WHERE id = ? AND status IN ('pending', 'processing')",
            )
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to mark trigger request processing: {e}")))
        })
        .await?;
        Ok(())
    }

    /// Record the run a request produced. Terminal.
    pub async fn mark_completed(
        pool: &Pool,
        id: Uuid,
        run_id: gitforge_common::PipelineRunId,
    ) -> Result<()> {
        persist_with_retry(|| async {
            sqlx::query(
                "UPDATE pipeline_trigger_requests \
                 SET status = 'completed', run_id = ?, error = NULL, updated_at = ? WHERE id = ?",
            )
            .bind(run_id.to_string())
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to complete trigger request: {e}")))
        })
        .await?;
        Ok(())
    }

    /// Record a terminal failure with its cause, and the run row the failure
    /// produced when one exists. Terminal — retry budget is the consumer's
    /// decision to spend by leaving the row `pending` instead; once here,
    /// the failure is visible instead of hollow.
    pub async fn mark_failed(
        pool: &Pool,
        id: Uuid,
        error: &str,
        run_id: Option<gitforge_common::PipelineRunId>,
    ) -> Result<()> {
        persist_with_retry(|| async {
            sqlx::query(
                "UPDATE pipeline_trigger_requests \
                 SET status = 'failed', error = ?, run_id = ?, updated_at = ? WHERE id = ?",
            )
            .bind(error)
            .bind(run_id.map(|id| id.to_string()))
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to fail trigger request: {e}")))
        })
        .await?;
        Ok(())
    }

    /// Return a request to `pending` after a retryable failure; the sweep
    /// republishes it once the backoff (the pending-age cutoff) elapses.
    pub async fn mark_retry(pool: &Pool, id: Uuid) -> Result<()> {
        persist_with_retry(|| async {
            sqlx::query(
                "UPDATE pipeline_trigger_requests \
                 SET status = 'pending', updated_at = ? WHERE id = ?",
            )
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(pool.pool())
            .await
            .map_err(|e| Error::database(format!("failed to retry trigger request: {e}")))
        })
        .await?;
        Ok(())
    }

    /// Recover interrupted requests: `pending` rows older than
    /// `pending_after` (their bus event was lost, or a retry's backoff
    /// elapsed) and `processing` rows older than `processing_after` (the
    /// consumer died mid-run-creation). Both are returned for republication
    /// and stamped `updated_at` so a sweep that outruns the consumer cannot
    /// republish in a tight loop. A crashed `processing` row is demoted back
    /// to `pending`; run creation itself is atomic, so a half-created run is
    /// impossible and the redelivery either finds the completed pipeline or
    /// creates it.
    pub async fn requeue_stale(
        pool: &Pool,
        pending_after: chrono::Duration,
        processing_after: chrono::Duration,
    ) -> Result<Vec<crate::models::PipelineTriggerRequest>> {
        let now = Utc::now();
        let pending_cutoff = (now - pending_after).to_rfc3339();
        let processing_cutoff = (now - processing_after).to_rfc3339();
        let mut transaction = begin_immediate(pool.pool(), "trigger requeue").await?;
        let stale_processing: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM pipeline_trigger_requests \
             WHERE status = 'processing' AND updated_at <= ?",
        )
        .bind(&processing_cutoff)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to list stale processing requests: {e}")))?;
        for id in &stale_processing {
            sqlx::query(
                "UPDATE pipeline_trigger_requests SET status = 'pending', updated_at = ? WHERE id = ?",
            )
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(|e| Error::database(format!("failed to requeue trigger request: {e}")))?;
        }
        let stale_pending: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM pipeline_trigger_requests \
             WHERE status = 'pending' AND updated_at <= ?",
        )
        .bind(&pending_cutoff)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|e| Error::database(format!("failed to list stale pending requests: {e}")))?;
        for id in &stale_pending {
            sqlx::query("UPDATE pipeline_trigger_requests SET updated_at = ? WHERE id = ?")
                .bind(now.to_rfc3339())
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(|e| {
                    Error::database(format!(
                        "failed to stamp trigger request for republish: {e}"
                    ))
                })?;
        }
        transaction
            .commit()
            .await
            .map_err(|e| Error::database(format!("failed to commit trigger requeue: {e}")))?;

        let mut requests = Vec::new();
        for id in stale_processing.into_iter().chain(stale_pending) {
            let uuid = Uuid::parse_str(&id)
                .map_err(|e| Error::database(format!("invalid trigger request id: {e}")))?;
            if let Some(request) = Self::get(pool, uuid).await? {
                requests.push(request);
            }
        }
        Ok(requests)
    }

    /// Requests that exhausted their attempts without producing a run —
    /// the set an operator (or a status endpoint) needs to see so a failure
    /// is never silent.
    pub async fn list_failed(
        pool: &Pool,
        limit: i64,
    ) -> Result<Vec<crate::models::PipelineTriggerRequest>> {
        let rows = sqlx::query(
            "SELECT * FROM pipeline_trigger_requests WHERE status = 'failed' \
             ORDER BY updated_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to list failed trigger requests: {e}")))?;
        rows.into_iter().map(hydrate_trigger_request).collect()
    }
}

fn hydrate_trigger_request(
    row: sqlx::sqlite::SqliteRow,
) -> Result<crate::models::PipelineTriggerRequest> {
    let run_id: Option<String> = row
        .try_get("run_id")
        .map_err(|error| Error::database(format!("invalid trigger request run_id: {error}")))?;
    Ok(crate::models::PipelineTriggerRequest {
        id: parse_uuid_column(&row, "id")?,
        repo_id: RepoId::from(parse_uuid_column(&row, "repo_id")?),
        ref_name: row
            .try_get("ref_name")
            .map_err(|error| Error::database(format!("invalid ref_name: {error}")))?,
        old_hash: row
            .try_get("old_hash")
            .map_err(|error| Error::database(format!("invalid old_hash: {error}")))?,
        new_hash: row
            .try_get("new_hash")
            .map_err(|error| Error::database(format!("invalid new_hash: {error}")))?,
        status: row
            .try_get("status")
            .map_err(|error| Error::database(format!("invalid status: {error}")))?,
        run_id: run_id
            .map(|id| {
                Uuid::parse_str(&id)
                    .map_err(|error| Error::database(format!("invalid run_id: {error}")))
            })
            .transpose()?
            .map(gitforge_common::PipelineRunId::from),
        attempts: row
            .try_get("attempts")
            .map_err(|error| Error::database(format!("invalid attempts: {error}")))?,
        error: row
            .try_get("error")
            .map_err(|error| Error::database(format!("invalid error: {error}")))?,
        created_at: parse_timestamp_column(&row, "created_at")?,
        updated_at: parse_timestamp_column(&row, "updated_at")?,
    })
}

// ============================================================================
// Refresh tokens
// ============================================================================

/// A stored refresh token row. Only the SHA-256 digest of the token is
/// kept: a database leak must not yield usable credentials, and the
/// plaintext exists only on the client that was handed it at
/// login/refresh time.
#[derive(Debug, Clone)]
pub struct RefreshTokenRow {
    pub user_id: UserId,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub struct RefreshTokenQueries;

impl RefreshTokenQueries {
    /// Store a refresh token (digest only) for a user.
    pub async fn create(
        pool: &Pool,
        user_id: UserId,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO refresh_tokens (id, user_id, token_hash, created_at, expires_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(user_id.to_string())
        .bind(token_hash)
        .bind(Utc::now().to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to create refresh token: {e}")))?;
        Ok(())
    }

    /// Look up a live (unrevoked, unexpired) token row by its hash.
    pub async fn find_active_by_hash(
        pool: &Pool,
        token_hash: &str,
    ) -> Result<Option<RefreshTokenRow>> {
        let row = sqlx::query(
            r#"
            SELECT user_id, token_hash, expires_at, revoked_at, created_at
            FROM refresh_tokens
            WHERE token_hash = ?
              AND revoked_at IS NULL
              AND expires_at > ?
            "#,
        )
        .bind(token_hash)
        .bind(Utc::now().to_rfc3339())
        .fetch_optional(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to look up refresh token: {e}")))?;

        let Some(row) = row else { return Ok(None) };
        let expires_at: String = row
            .try_get("expires_at")
            .map_err(|error| Error::database(format!("invalid expires_at: {error}")))?;
        Ok(Some(RefreshTokenRow {
            user_id: UserId::from(parse_uuid_column(&row, "user_id")?),
            token_hash: row
                .try_get("token_hash")
                .map_err(|error| Error::database(format!("invalid token_hash: {error}")))?,
            expires_at: DateTime::parse_from_rfc3339(&expires_at)
                .map_err(|error| Error::database(format!("invalid expires_at: {error}")))?
                .with_timezone(&Utc),
            revoked_at: row
                .try_get::<Option<String>, _>("revoked_at")
                .map_err(|error| Error::database(format!("invalid revoked_at: {error}")))?
                .map(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .map_err(|error| Error::database(format!("invalid revoked_at: {error}")))
                })
                .transpose()?
                .map(|value| value.with_timezone(&Utc)),
            created_at: parse_timestamp_column(&row, "created_at")?,
        }))
    }

    /// Revoke a token by hash. Idempotent: revoking an already-revoked (or
    /// unknown) token is not an error — logout must never fail open.
    pub async fn revoke(pool: &Pool, token_hash: &str) -> Result<()> {
        sqlx::query(
            "UPDATE refresh_tokens SET revoked_at = ? WHERE token_hash = ? AND revoked_at IS NULL",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(token_hash)
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to revoke refresh token: {e}")))?;
        Ok(())
    }

    /// Revoke every live token for a user (password change, admin lockout).
    pub async fn revoke_all_for_user(pool: &Pool, user_id: UserId) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE refresh_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(user_id.to_string())
        .execute(pool.pool())
        .await
        .map_err(|e| Error::database(format!("failed to revoke user refresh tokens: {e}")))?;
        Ok(result.rows_affected())
    }

    /// Drop expired/revoked rows older than the cutoff. Housekeeping only —
    /// revoked rows could stay forever without correctness impact.
    pub async fn prune(pool: &Pool, older_than: DateTime<Utc>) -> Result<u64> {
        let result =
            sqlx::query("DELETE FROM refresh_tokens WHERE (expires_at < ?1 OR revoked_at < ?1)")
                .bind(older_than.to_rfc3339())
                .execute(pool.pool())
                .await
                .map_err(|e| Error::database(format!("failed to prune refresh tokens: {e}")))?;
        Ok(result.rows_affected())
    }
}

// ============================================================================
// Dashboard aggregate stats
// ============================================================================

/// One scalar aggregate, rendered as a dashboard metric.
///
/// Counts are read with plain `COUNT(*)` outside any transaction: the
/// dashboard is an approximation surface, not an invariant surface, and
/// a mid-count concurrent write costs nothing (the next render corrects
/// it). Every query is single-pass over an indexed-or-tiny table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardStats {
    pub repositories: i64,
    pub pipelines: i64,
    pub artifacts: i64,
    pub runners_online: i64,
    pub runs_last_24h: i64,
    pub runs_succeeded_24h: i64,
}

pub struct StatsQueries;

impl StatsQueries {
    /// Read every dashboard aggregate in one round trip per scalar.
    ///
    /// Timestamps are stored as RFC3339 strings (`to_rfc3339()` on every
    /// write path), so the 24h window is bounded by an RFC3339 bound —
    /// not SQLite's space-separated `datetime('now', ...)`, which
    /// compares wrongly against the `T` separator inside the same day.
    pub async fn dashboard(pool: &Pool) -> Result<DashboardStats> {
        let count = |sql: &'static str| {
            let pool = pool;
            async move {
                let row = sqlx::query(sql)
                    .fetch_one(pool.pool())
                    .await
                    .map_err(|e| Error::database(format!("dashboard count failed: {e}")))?;
                Ok::<i64, Error>(row.get::<i64, _>(0))
            }
        };
        let window_bound = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
        let run_row = sqlx::query(
            r#"
            SELECT
                COUNT(*) AS total,
                SUM(CASE WHEN status = 'succeeded' THEN 1 ELSE 0 END) AS succeeded
            FROM pipeline_runs
            WHERE created_at > ?
            "#,
        )
        .bind(window_bound)
        .fetch_one(pool.pool())
        .await
        .map_err(|e| Error::database(format!("dashboard run stats failed: {e}")))?;
        Ok(DashboardStats {
            repositories: count("SELECT COUNT(*) FROM repositories").await?,
            pipelines: count("SELECT COUNT(*) FROM pipelines").await?,
            artifacts: count("SELECT COUNT(*) FROM artifacts").await?,
            runners_online: count("SELECT COUNT(*) FROM runners WHERE status = 'online'").await?,
            runs_last_24h: run_row.get::<i64, _>("total"),
            runs_succeeded_24h: run_row.get::<Option<i64>, _>("succeeded").unwrap_or(0),
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dashboard_stats_count_seeded_rows() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let now = Utc::now().to_rfc3339();
        let old = (Utc::now() - chrono::Duration::hours(48)).to_rfc3339();
        let insert = |sql: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(sql).execute(pool.pool()).await.unwrap();
            }
        };
        // Two repositories, one pipeline, one artifact, two runners with
        // only one online, and four runs: two succeeded plus one failed
        // inside the 24h window, one succeeded outside it.
        for sql in [
            "INSERT INTO repositories (id, name, owner_id, git_path, created_at, updated_at)
             VALUES ('r1', 'a', 'u1', '/tmp/a.git', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00'),
                    ('r2', 'b', 'u1', '/tmp/b.git', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
            "INSERT INTO pipelines (id, repo_id, name, trigger_type, created_at)
             VALUES ('p1', 'r1', 'ci', 'push', '2026-01-01T00:00:00+00:00')",
            "INSERT INTO artifacts (id, job_id, name, path, checksum, size_bytes, created_at)
             VALUES ('a1', 'j1', 'out.zip', '/tmp/out.zip', 'ck', 1, '2026-01-01T00:00:00+00:00')",
            "INSERT INTO runners (id, name, runner_type, status, created_at, updated_at)
             VALUES ('n1', 'runner-1', 'docker', 'online', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00'),
                    ('n2', 'runner-2', 'docker', 'offline', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
        ] {
            insert(sql).await;
        }
        sqlx::query(
            "INSERT INTO pipeline_runs
                 (id, pipeline_id, repo_id, status, triggered_by, commit_hash, created_at)
             VALUES ('run1', 'p1', 'r1', 'succeeded', 'push', 'c1', ?1),
                    ('run2', 'p1', 'r1', 'succeeded', 'push', 'c2', ?1),
                    ('run3', 'p1', 'r1', 'failed', 'push', 'c3', ?1),
                    ('run4', 'p1', 'r1', 'succeeded', 'push', 'c4', ?2)",
        )
        .bind(now)
        .bind(old)
        .execute(pool.pool())
        .await
        .unwrap();
        // The artifact row carries a real foreign key into jobs.
        sqlx::query(
            "INSERT INTO jobs (id, pipeline_run_id, name, created_at)
             VALUES ('j1', 'run1', 'fmt', '2026-01-01T00:00:00+00:00')",
        )
        .execute(pool.pool())
        .await
        .unwrap();

        let stats = StatsQueries::dashboard(&pool).await.unwrap();
        assert_eq!(stats.repositories, 2);
        assert_eq!(stats.pipelines, 1);
        assert_eq!(stats.artifacts, 1);
        assert_eq!(stats.runners_online, 1);
        // The 48h-old succeeded run is outside the window and must not
        // count in either the total or the success rate.
        assert_eq!(stats.runs_last_24h, 3);
        assert_eq!(stats.runs_succeeded_24h, 2);
    }

    #[tokio::test]
    async fn test_dashboard_stats_empty_database_is_all_zero() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let stats = StatsQueries::dashboard(&pool).await.unwrap();
        assert_eq!(
            stats,
            DashboardStats {
                repositories: 0,
                pipelines: 0,
                artifacts: 0,
                runners_online: 0,
                runs_last_24h: 0,
                runs_succeeded_24h: 0,
            }
        );
    }

    #[tokio::test]
    async fn test_persist_with_retry_succeeds_after_transient_failures() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<&'static str> = persist_with_retry(|| {
            let count = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                if count < 2 {
                    Err(Error::database("database is busy"))
                } else {
                    Ok("committed")
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), "committed");
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_persist_with_retry_propagates_non_database_errors() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let result: Result<()> = persist_with_retry(|| {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { Err(Error::not_found("job", uuid::Uuid::new_v4())) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a non-database error must not be retried"
        );
    }

    /// Regression for the 2026-10-08 completion failures: a pooled
    /// connection whose SQLite handle is inside a transaction that sqlx's
    /// depth counter does not know about poisons every subsequent
    /// `begin_with` with `(code: 1) cannot start a transaction within a
    /// transaction`, and the pool hands the same connection back to each
    /// retry. `begin_immediate` must clear the desync with a bare ROLLBACK
    /// and yield a usable transaction.
    ///
    /// The pool is capped at one connection so every borrower deterministically
    /// gets the poisoned one; the poison itself is injected with a raw BEGIN,
    /// which bypasses sqlx's depth counter exactly like the live desync did.
    #[tokio::test]
    async fn test_begin_immediate_recovers_desynced_connection() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        // Poison the pool's only connection.
        let mut conn = pool.acquire().await.unwrap();
        sqlx::raw_sql("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        // Sanity: the plain begin now fails with the live error signature.
        let poisoned = pool.begin_with("BEGIN IMMEDIATE").await;
        let message = format!("{}", poisoned.unwrap_err());
        assert!(
            message.contains("within a transaction"),
            "expected the desynced begin to fail with the nested-transaction \
             error, got: {message}"
        );

        // Recovery: the begin must succeed and the transaction must work.
        let mut tx = begin_immediate(&pool, "completion").await.unwrap();
        sqlx::query("CREATE TABLE poison_probe (id INTEGER)")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // And the pool keeps serving healthy transactions afterwards.
        let mut tx = begin_immediate(&pool, "completion").await.unwrap();
        sqlx::query("INSERT INTO poison_probe (id) VALUES (1)")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        pool.close().await;
    }

    /// Regression for the swallowed-write flavor of connection poisoning
    /// (run `99c463db`, 2026-10-08): a single-statement write routed in
    /// autocommit to a connection with an orphaned SQLite transaction
    /// joins that transaction and vanishes when it rolls back — the write
    /// reports success and the row is gone. The pool's `before_acquire`
    /// hook must heal the connection before the statement runs.
    ///
    /// Cross-connection truth is what makes this observable: the poisoned
    /// transaction's uncommitted DDL is visible on its own connection, so
    /// the probe closes the pool and reopens the file to check whether the
    /// write actually committed. The no-hook variant documents the failure
    /// mode this guards against.
    #[tokio::test]
    async fn test_before_acquire_heals_swallowed_writes() {
        let db_path = std::env::temp_dir().join(format!("gitforge-swallow-{}.db", Uuid::new_v4()));
        let url = format!("sqlite:{}", db_path.display());

        // Poisoned pool WITHOUT the hook: the write is swallowed.
        let bare = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut conn = bare.acquire().await.unwrap();
        sqlx::raw_sql("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        sqlx::query("CREATE TABLE swallow_probe (id INTEGER)")
            .execute(&bare)
            .await
            .unwrap();
        bare.close().await;

        let reopened = sqlx::sqlite::SqlitePoolOptions::new()
            .connect(&url)
            .await
            .unwrap();
        let leaked: Option<i64> =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = 'swallow_probe'")
                .fetch_one(&reopened)
                .await
                .unwrap();
        reopened.close().await;
        assert_eq!(
            leaked,
            Some(0),
            "the unhooked pool must demonstrate the swallowed write"
        );

        // Same shape WITH the hook (what Pool::new installs): healed at
        // acquire, so the write commits and survives the reopen.
        let hooked = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .before_acquire(|conn, _meta| crate::connection::heal_poisoned_connection(conn))
            .connect(&url)
            .await
            .unwrap();
        let mut conn = hooked.acquire().await.unwrap();
        sqlx::raw_sql("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        sqlx::query("CREATE TABLE swallow_probe (id INTEGER)")
            .execute(&hooked)
            .await
            .unwrap();
        hooked.close().await;

        let reopened = sqlx::sqlite::SqlitePoolOptions::new()
            .connect(&url)
            .await
            .unwrap();
        let committed: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = 'swallow_probe'")
                .fetch_one(&reopened)
                .await
                .unwrap();
        reopened.close().await;
        assert_eq!(
            committed, 1,
            "the hooked pool must heal the poison so the write commits"
        );

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    }

    #[tokio::test]
    async fn test_cancel_is_idempotent_and_never_clobbers_a_terminal_job() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let owner = crate::models::User::new(
            "cancel-owner".to_string(),
            "cancel-owner@example.com".to_string(),
            "hash".to_string(),
        );
        crate::queries::UserQueries::create(&pool, &owner)
            .await
            .unwrap();
        let repo = crate::models::Repository::new(
            "cancel-repo".to_string(),
            owner.id,
            "/git/cancel-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();
        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "cancel-pipeline".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({"jobs": []}),
            created_at: Utc::now(),
        };
        PipelineQueries::create(&pool, &pipeline).await.unwrap();
        let run = crate::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "push".to_string(),
            "0".repeat(40),
        );
        PipelineRunQueries::create(&pool, &run).await.unwrap();

        let job = crate::models::Job::new(run.id, "cancel-target".to_string());
        JobQueries::create(&pool, &job).await.unwrap();
        JobQueries::cancel(&pool, job.id, r#"{"reason":"first"}"#)
            .await
            .unwrap();
        // Second cancel of the already-terminal row is a success and must
        // not rewrite the receipt.
        JobQueries::cancel(&pool, job.id, r#"{"reason":"second"}"#)
            .await
            .unwrap();
        let stored = JobQueries::get(&pool, job.id).await.unwrap().unwrap();
        assert_eq!(stored.status, "cancelled");
        assert!(stored.result_json.unwrap().contains("first"));

        // A completed job is likewise never dragged back to cancelled.
        let completed = crate::models::Job::new(run.id, "completed-target".to_string());
        JobQueries::create(&pool, &completed).await.unwrap();
        JobQueries::update_status(&pool, completed.id, "succeeded")
            .await
            .unwrap();
        JobQueries::cancel(&pool, completed.id, r#"{"reason":"late"}"#)
            .await
            .unwrap();
        let stored = JobQueries::get(&pool, completed.id).await.unwrap().unwrap();
        assert_eq!(stored.status, "succeeded");

        // Cancelling a job that does not exist is a not-found, not a lie.
        let missing = JobQueries::cancel(&pool, JobId::new(), "{}").await;
        assert!(missing.is_err());
    }

    #[tokio::test]
    async fn test_activate_pipeline_and_create_run_is_atomic() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let owner = crate::models::User::new(
            "atomic-owner".to_string(),
            "atomic-owner@example.com".to_string(),
            "hash".to_string(),
        );
        crate::queries::UserQueries::create(&pool, &owner)
            .await
            .unwrap();
        let repo = crate::models::Repository::new(
            "atomic-repo".to_string(),
            owner.id,
            "/git/atomic-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();

        let first = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "ci".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({"version": 1}),
            created_at: Utc::now(),
        };
        PipelineQueries::create(&pool, &first).await.unwrap();

        // Push 2 retires push 1's version and lands its own in one step.
        let second = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "ci".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({"version": 2}),
            created_at: Utc::now(),
        };
        let run =
            crate::models::PipelineRun::new(second.id, repo.id, "push".to_string(), "1".repeat(40));
        let mut started = run.clone();
        started.start();
        PipelineQueries::activate_pipeline_and_create_run(&pool, &second, &started)
            .await
            .unwrap();

        assert_eq!(
            PipelineQueries::count_active(&pool, repo.id, "ci")
                .await
                .unwrap(),
            1,
            "exactly one active version survives the swap"
        );
        let stored_run = PipelineRunQueries::get(&pool, run.id)
            .await
            .unwrap()
            .expect("run persisted in the same transaction");
        assert_eq!(stored_run.status, "running");
        assert!(stored_run.started_at.is_some());
    }

    #[tokio::test]
    async fn test_trigger_request_dedup_lifecycle() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let owner = crate::models::User::new(
            "trigger-owner".to_string(),
            "trigger-owner@example.com".to_string(),
            "hash".to_string(),
        );
        crate::queries::UserQueries::create(&pool, &owner)
            .await
            .unwrap();
        let repo = crate::models::Repository::new(
            "trigger-repo".to_string(),
            owner.id,
            "/git/trigger-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();

        let request = crate::models::PipelineTriggerRequest::new(
            repo.id,
            "refs/heads/main".to_string(),
            "0".repeat(40),
            "a".repeat(40),
        );
        let (created, was_new) = TriggerRequestQueries::create_or_existing(&pool, &request)
            .await
            .unwrap();
        assert!(was_new);

        // A racing trigger for the same commit dedups onto the same row.
        let duplicate = crate::models::PipelineTriggerRequest::new(
            repo.id,
            "refs/heads/other".to_string(),
            "0".repeat(40),
            "a".repeat(40),
        );
        let (existing, was_new) = TriggerRequestQueries::create_or_existing(&pool, &duplicate)
            .await
            .unwrap();
        assert!(!was_new);
        assert_eq!(existing.id, created.id);

        // Lifecycle: pickup counts an attempt, completion records the run.
        TriggerRequestQueries::mark_processing(&pool, created.id)
            .await
            .unwrap();
        let run_id = PipelineRunId::new();
        TriggerRequestQueries::mark_completed(&pool, created.id, run_id)
            .await
            .unwrap();
        let finished = TriggerRequestQueries::get(&pool, created.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(finished.status, "completed");
        assert_eq!(
            finished.run_id.map(|id| id.to_string()),
            Some(run_id.to_string())
        );
        assert_eq!(finished.attempts, 1);

        // A completed row no longer dedups: an explicit re-trigger of the
        // same commit mints a fresh request.
        let (_, was_new) = TriggerRequestQueries::create_or_existing(&pool, &duplicate)
            .await
            .unwrap();
        assert!(was_new);
    }

    #[tokio::test]
    async fn test_trigger_request_requeue_stale_and_terminal_failure() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let owner = crate::models::User::new(
            "requeue-owner".to_string(),
            "requeue-owner@example.com".to_string(),
            "hash".to_string(),
        );
        crate::queries::UserQueries::create(&pool, &owner)
            .await
            .unwrap();
        let repo = crate::models::Repository::new(
            "requeue-repo".to_string(),
            owner.id,
            "/git/requeue-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();

        // A request whose consumer died mid-processing (row stuck in
        // `processing`, updated_at in the past) is demoted back to pending.
        let stuck = crate::models::PipelineTriggerRequest::new(
            repo.id,
            "refs/heads/main".to_string(),
            "0".repeat(40),
            "b".repeat(40),
        );
        let (stuck, _) = TriggerRequestQueries::create_or_existing(&pool, &stuck)
            .await
            .unwrap();
        TriggerRequestQueries::mark_processing(&pool, stuck.id)
            .await
            .unwrap();
        sqlx::query("UPDATE pipeline_trigger_requests SET updated_at = ? WHERE id = ?")
            .bind((Utc::now() - chrono::Duration::minutes(10)).to_rfc3339())
            .bind(stuck.id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        let requeued = TriggerRequestQueries::requeue_stale(
            &pool,
            chrono::Duration::minutes(5),
            chrono::Duration::minutes(5),
        )
        .await
        .unwrap();
        assert_eq!(requeued.len(), 1);
        assert_eq!(requeued[0].id, stuck.id);
        assert_eq!(requeued[0].status, "pending");

        // A fresh row inside the cutoffs is not touched.
        let fresh = crate::models::PipelineTriggerRequest::new(
            repo.id,
            "refs/heads/dev".to_string(),
            "0".repeat(40),
            "c".repeat(40),
        );
        let (fresh, _) = TriggerRequestQueries::create_or_existing(&pool, &fresh)
            .await
            .unwrap();
        let requeued = TriggerRequestQueries::requeue_stale(
            &pool,
            chrono::Duration::minutes(5),
            chrono::Duration::minutes(5),
        )
        .await
        .unwrap();
        assert!(requeued.iter().all(|request| request.id != fresh.id));

        // Terminal failure is visible and final.
        TriggerRequestQueries::mark_failed(&pool, fresh.id, "invalid .gitforce.yml", None)
            .await
            .unwrap();
        let failed = TriggerRequestQueries::list_failed(&pool, 10).await.unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].id, fresh.id);
        assert_eq!(failed[0].error.as_deref(), Some("invalid .gitforce.yml"));
    }

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
    async fn test_repo_policy_roundtrip() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let user = crate::models::User::new(
            "policy-owner".to_string(),
            "policy-owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();

        let repo = crate::models::Repository::new(
            "policy-repo".to_string(),
            user.id,
            "/git/policy-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();

        // Fresh repositories ship with no ref-update policy so existing
        // push flows are unchanged until an operator opts in (#240).
        let found = RepoQueries::get(&pool, repo.id).await.unwrap().unwrap();
        assert!(found.required_checks.is_empty());
        assert!(!found.deny_non_fast_forward);

        RepoQueries::update_policy(
            &pool,
            repo.id,
            &["ci".to_string(), "gates-and-release".to_string()],
            true,
        )
        .await
        .unwrap();

        let found = RepoQueries::get(&pool, repo.id).await.unwrap().unwrap();
        assert_eq!(found.required_checks, vec!["ci", "gates-and-release"]);
        assert!(found.deny_non_fast_forward);
        assert!(found.has_required_checks());

        // Clearing the policy round-trips too.
        RepoQueries::update_policy(&pool, repo.id, &[], false)
            .await
            .unwrap();
        let found = RepoQueries::get(&pool, repo.id).await.unwrap().unwrap();
        assert!(found.required_checks.is_empty());
        assert!(!found.deny_non_fast_forward);

        // Policy updates for an unknown repository are an error, not a no-op.
        assert!(RepoQueries::update_policy(&pool, RepoId::new(), &[], false)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_list_commit_statuses_for_ref_policy() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let user = crate::models::User::new(
            "status-owner".to_string(),
            "status-owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();

        let repo = crate::models::Repository::new(
            "status-repo".to_string(),
            user.id,
            "/git/status-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();

        let ci = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "ci".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        PipelineQueries::create(&pool, &ci).await.unwrap();
        let release = crate::models::Pipeline {
            id: PipelineId::new(),
            name: "gates-and-release".to_string(),
            ..ci.clone()
        };
        PipelineQueries::create(&pool, &release).await.unwrap();

        // Two ci runs on the same commit; the older one is backdated so the
        // "latest run wins" rule is exercised deterministically.
        let old_run = crate::models::PipelineRun::new(
            ci.id,
            repo.id,
            "push".to_string(),
            "ABCDEF1234".to_string(),
        );
        PipelineRunQueries::create(&pool, &old_run).await.unwrap();
        PipelineRunQueries::update_status(&pool, old_run.id, "failed")
            .await
            .unwrap();
        let newer_run = crate::models::PipelineRun::new(
            ci.id,
            repo.id,
            "push".to_string(),
            "abcdef1234".to_string(),
        );
        PipelineRunQueries::create(&pool, &newer_run).await.unwrap();
        PipelineRunQueries::update_status(&pool, newer_run.id, "succeeded")
            .await
            .unwrap();
        sqlx::query("UPDATE pipeline_runs SET created_at = ? WHERE id = ?")
            .bind((chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339())
            .bind(old_run.id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        let release_run = crate::models::PipelineRun::new(
            release.id,
            repo.id,
            "push".to_string(),
            "abcdef1234".to_string(),
        );
        // PipelineRun::new starts pending — the release gate has not run yet.
        assert_eq!(release_run.status, "pending");
        PipelineRunQueries::create(&pool, &release_run)
            .await
            .unwrap();

        // Hash match is case-insensitive: the push path and historical rows
        // may disagree about hex case.
        let statuses = PipelineRunQueries::list_commit_statuses(&pool, repo.id, "AbCdEf1234")
            .await
            .unwrap();
        assert_eq!(statuses.len(), 3);

        let checks = evaluate_required_checks(
            &[
                "ci".to_string(),
                "gates-and-release".to_string(),
                "docs".to_string(),
            ],
            &statuses,
        );
        assert_eq!(checks.len(), 3);
        let ci_check = checks.iter().find(|c| c.check == "ci").unwrap();
        assert_eq!(ci_check.status.as_deref(), Some("succeeded"));
        assert!(ci_check.is_green());
        let release_check = checks
            .iter()
            .find(|c| c.check == "gates-and-release")
            .unwrap();
        assert_eq!(release_check.status.as_deref(), Some("pending"));
        assert!(!release_check.is_green());
        let missing = checks.iter().find(|c| c.check == "docs").unwrap();
        assert_eq!(missing.status, None);
        assert!(!missing.is_green());
        assert!(!required_checks_satisfied(&checks));

        // A commit with no runs yields no statuses and a failing gate.
        let none = PipelineRunQueries::list_commit_statuses(&pool, repo.id, &"0".repeat(40))
            .await
            .unwrap();
        assert!(none.is_empty());
        assert!(!required_checks_satisfied(&evaluate_required_checks(
            &["ci".to_string()],
            &none
        )));
    }

    #[test]
    fn test_required_checks_satisfied_rules() {
        let green = |check: &str| RequiredCheckStatus {
            check: check.to_string(),
            status: Some("succeeded".to_string()),
        };
        let red = |check: &str, status: &str| RequiredCheckStatus {
            check: check.to_string(),
            status: Some(status.to_string()),
        };

        // No policy configured: nothing blocks.
        assert!(required_checks_satisfied(&[]));
        assert!(required_checks_satisfied(&[green("ci")]));
        for status in ["pending", "running", "failed", "cancelled", "timed_out"] {
            assert!(!required_checks_satisfied(&[red("ci", status)]));
        }

        // Latest run wins: an older red run behind a newer green one must not
        // fail the gate (list_commit_statuses orders latest-first, and the
        // aggregator ignores duplicates after the first).
        let statuses = [
            CommitRunStatus {
                pipeline: "ci".to_string(),
                status: Some("succeeded".to_string()),
            },
            CommitRunStatus {
                pipeline: "ci".to_string(),
                status: Some("failed".to_string()),
            },
        ];
        let checks = evaluate_required_checks(&["ci".to_string()], &statuses);
        assert!(checks[0].is_green());
        assert!(required_checks_satisfied(&checks));
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

        sqlx::query("UPDATE users SET created_at = ? WHERE id = ?")
            .bind("2026-08-29 03:40:39")
            .bind(user.id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();
        assert!(UserQueries::get(&pool, user.id).await.is_err());
        assert!(UserQueries::list(&pool).await.is_err());
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

        let user = crate::models::User::new(
            "runner-owner".to_string(),
            "runner-owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();
        let repo = crate::models::Repository::new(
            "runner-repo".to_string(),
            user.id,
            "/git/runner-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();
        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "runner-pipeline".to_string(),
            trigger_type: "manual".to_string(),
            config: serde_json::json!({}),
            created_at: Utc::now(),
        };
        PipelineQueries::create(&pool, &pipeline).await.unwrap();
        let run = crate::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            user.username.clone(),
            "runner-commit".to_string(),
        );
        PipelineRunQueries::create(&pool, &run).await.unwrap();
        let job = crate::models::Job::new(run.id, "runner-job".to_string());
        JobQueries::create(&pool, &job).await.unwrap();
        JobQueries::assign(&pool, job.id, runner.id).await.unwrap();
        assert_eq!(
            RunnerQueries::retire_if_idle(&pool, runner.id)
                .await
                .unwrap(),
            RunnerRetirement::ActiveJobs(1)
        );
        JobQueries::complete(&pool, job.id, "succeeded", "{}")
            .await
            .unwrap();

        assert_eq!(
            RunnerQueries::retire_if_idle(&pool, runner.id)
                .await
                .unwrap(),
            RunnerRetirement::Retired
        );
        assert_eq!(
            RunnerQueries::get(&pool, runner.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "retired"
        );
        assert_eq!(
            RunnerQueries::retire_if_idle(&pool, runner.id)
                .await
                .unwrap(),
            RunnerRetirement::AlreadyRetired
        );
        assert_eq!(
            RunnerQueries::retire_if_idle(&pool, RunnerId::new())
                .await
                .unwrap(),
            RunnerRetirement::NotFound
        );
    }

    #[tokio::test]
    async fn test_runner_registration_refreshes_existing_name() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let first = crate::models::Runner::new(
            "stable-runner".to_string(),
            crate::models::RunnerType::Docker,
            2,
        );
        let (registered, outcome) = RunnerQueries::register_or_refresh(&pool, &first)
            .await
            .unwrap();
        assert_eq!(outcome, RunnerRegistration::Created);

        let mut restarted = crate::models::Runner::new(
            "stable-runner".to_string(),
            crate::models::RunnerType::Docker,
            4,
        );
        restarted.set_busy();
        let (refreshed, outcome) = RunnerQueries::register_or_refresh(&pool, &restarted)
            .await
            .unwrap();
        assert_eq!(outcome, RunnerRegistration::Refreshed);

        assert_eq!(refreshed.id, registered.id);
        assert_eq!(refreshed.capacity, 4);
        assert_eq!(refreshed.status, "busy");
        assert_eq!(RunnerQueries::list(&pool).await.unwrap().len(), 1);
    }

    /// Concurrent registrations of one name must converge on a single row.
    /// The SELECT-then-INSERT race window is closed by the unique index on
    /// runners(name); losers of the insert race fall through to the refresh
    /// path, so every caller succeeds and the registry stays honest.
    #[tokio::test]
    async fn test_runner_registration_converges_under_concurrency() {
        let pool = std::sync::Arc::new(Pool::memory().await.unwrap());
        pool.migrate().await.unwrap();

        let mut handles = Vec::new();
        for attempt in 0..8 {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                let runner = crate::models::Runner::new(
                    "shared-runner".to_string(),
                    crate::models::RunnerType::Docker,
                    attempt + 1,
                );
                RunnerQueries::register_or_refresh(&pool, &runner).await
            }));
        }

        let mut ids = std::collections::HashSet::new();
        for handle in handles {
            let (runner, _) = handle.await.unwrap().unwrap();
            ids.insert(runner.id);
        }

        assert_eq!(ids.len(), 1, "all registrations must adopt one row");
        assert_eq!(
            RunnerQueries::list(&pool).await.unwrap().len(),
            1,
            "registry must hold exactly one row for the name"
        );
    }

    /// The unique index is the backstop: even a raw duplicate insert is
    /// rejected, and migration renames pre-existing duplicates instead of
    /// dropping their audit records.
    #[tokio::test]
    async fn test_runner_name_unique_index_rejects_duplicates() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let first = crate::models::Runner::new(
            "indexed-runner".to_string(),
            crate::models::RunnerType::Docker,
            1,
        );
        RunnerQueries::create(&pool, &first).await.unwrap();

        let duplicate = crate::models::Runner::new(
            "indexed-runner".to_string(),
            crate::models::RunnerType::Docker,
            1,
        );
        let error = RunnerQueries::create(&pool, &duplicate)
            .await
            .expect_err("duplicate name must be rejected by the unique index");
        assert!(
            error.message.contains("UNIQUE constraint failed"),
            "unexpected error: {error}"
        );
    }

    /// Migration must heal databases that accumulated one row per runner
    /// restart before names were unique: the newest row keeps the name and
    /// older duplicates are renamed (audit preserved), then the index holds.
    #[tokio::test]
    async fn test_migration_renames_duplicate_runner_names() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        // Simulate a pre-index database: drop the index, insert duplicates.
        sqlx::query("DROP INDEX idx_runners_name")
            .execute(pool.pool())
            .await
            .unwrap();
        let now = Utc::now().to_rfc3339();
        let older = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        for (id, updated) in [
            ("11111111-1111-1111-1111-111111111111", older),
            ("22222222-2222-2222-2222-222222222222", Utc::now()),
        ] {
            sqlx::query(
                "INSERT INTO runners (id, name, runner_type, status, capacity, labels, last_heartbeat, created_at, updated_at) \
                 VALUES (?, 'dup-runner', 'docker', 'offline', 1, '[]', NULL, ?, ?)",
            )
            .bind(id)
            .bind(now.clone())
            .bind(updated.to_rfc3339())
            .execute(pool.pool())
            .await
            .unwrap();
        }

        // Re-running migration deduplicates and recreates the index.
        pool.migrate().await.unwrap();

        let runners = RunnerQueries::list(&pool).await.unwrap();
        assert_eq!(runners.len(), 2, "audit records must be preserved");
        let kept: Vec<_> = runners.iter().filter(|r| r.name == "dup-runner").collect();
        assert_eq!(kept.len(), 1, "newest row keeps the operator-facing name");
        assert_eq!(
            kept[0].id.to_string(),
            "22222222-2222-2222-2222-222222222222"
        );
        let legacy: Vec<_> = runners
            .iter()
            .filter(|r| r.name.contains("-legacy-"))
            .collect();
        assert_eq!(legacy.len(), 1);
        assert!(legacy[0].name.starts_with("dup-runner-legacy-"));

        // The boot after deduplication must not touch the rows again: the
        // migration skips its write entirely when no duplicates remain, so
        // restarts stay read-only under concurrent writer churn.
        pool.migrate().await.unwrap();
        let runners_after = RunnerQueries::list(&pool).await.unwrap();
        let after_names: std::collections::HashSet<_> =
            runners_after.iter().map(|r| r.name.clone()).collect();
        let before_names: std::collections::HashSet<_> =
            runners.iter().map(|r| r.name.clone()).collect();
        assert_eq!(before_names, after_names, "second boot is a no-op");
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
    async fn test_pipeline_list_rejects_malformed_timestamp_without_panicking() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = crate::models::User::new(
            "owner".to_string(),
            "owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();
        let repo = crate::models::Repository::new(
            "malformed-repo".to_string(),
            user.id,
            "/git/malformed-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();
        sqlx::query(
            "INSERT INTO pipelines (id, repo_id, name, trigger_type, config, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(PipelineId::new().to_string())
        .bind(repo.id.to_string())
        .bind("malformed")
        .bind("push")
        .bind("{}")
        .bind("2026-08-29 03:40:39")
        .execute(pool.pool())
        .await
        .unwrap();

        assert!(PipelineQueries::list_by_repo(&pool, repo.id).await.is_err());
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

        sqlx::query("UPDATE pipeline_runs SET created_at = ? WHERE id = ?")
            .bind("2026-08-29 03:40:39")
            .bind(run.id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();
        assert!(PipelineRunQueries::get(&pool, run.id).await.is_err());
    }

    #[tokio::test]
    async fn test_pipeline_run_failure_reason_persists() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

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
            "push".to_string(),
            "abc123".to_string(),
        );
        PipelineRunQueries::create(&pool, &run).await.unwrap();

        // A fresh run carries no failure reason.
        let found = PipelineRunQueries::get(&pool, run.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.error, None);

        // Failing with a reason records it alongside the terminal verdict.
        let reason = "workspace clone exceeded its time budget and was killed";
        PipelineRunQueries::update_status_with_error(&pool, run.id, "failed", Some(reason))
            .await
            .unwrap();
        let found = PipelineRunQueries::get(&pool, run.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, "failed");
        assert_eq!(found.error.as_deref(), Some(reason));
        assert!(found.finished_at.is_some());

        // A reason-less rewrite of the same status never erases the cause.
        PipelineRunQueries::update_status(&pool, run.id, "failed")
            .await
            .unwrap();
        let found = PipelineRunQueries::get(&pool, run.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.error.as_deref(), Some(reason));

        // The terminal-verdict guard still holds with the error variant.
        PipelineRunQueries::update_status_with_error(&pool, run.id, "succeeded", None)
            .await
            .unwrap();
        let found = PipelineRunQueries::get(&pool, run.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.status, "failed");
        assert_eq!(found.error.as_deref(), Some(reason));
    }

    #[tokio::test]
    async fn test_job_queries_cancel_unclaimed_for_run() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

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
            "push".to_string(),
            "abc123".to_string(),
        );
        PipelineRunQueries::create(&pool, &run).await.unwrap();

        let pending = crate::models::Job::new(run.id, "pending-job".to_string());
        let mut queued = crate::models::Job::new(run.id, "queued-job".to_string());
        queued.status = JobStatus::Queued.as_str().to_string();
        let running = crate::models::Job::new(run.id, "running-job".to_string());
        let done = crate::models::Job::new(run.id, "done-job".to_string());
        for job in [&pending, &queued, &running, &done] {
            JobQueries::create(&pool, job).await.unwrap();
        }
        JobQueries::update_status(&pool, running.id, "running")
            .await
            .unwrap();
        JobQueries::update_status(&pool, done.id, "succeeded")
            .await
            .unwrap();

        // Only the runner-untouched rows are swept.
        let swept = JobQueries::cancel_unclaimed_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(swept, 2);

        let swept_row = JobQueries::get(&pool, pending.id).await.unwrap().unwrap();
        assert_eq!(swept_row.status, "cancelled");
        assert!(swept_row.finished_at.is_some());
        let swept_row = JobQueries::get(&pool, queued.id).await.unwrap().unwrap();
        assert_eq!(swept_row.status, "cancelled");
        let untouched = JobQueries::get(&pool, running.id).await.unwrap().unwrap();
        assert_eq!(untouched.status, "running");
        let untouched = JobQueries::get(&pool, done.id).await.unwrap().unwrap();
        assert_eq!(untouched.status, "succeeded");

        // A second sweep over an already-terminal run is a no-op.
        let swept = JobQueries::cancel_unclaimed_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(swept, 0);
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

        JobQueries::set_definition_with_image_and_timeout(
            &pool,
            job.id,
            &["cargo test".to_string()],
            "rust:latest",
            None,
            900,
        )
        .await
        .unwrap();
        let configured = JobQueries::get(&pool, job.id).await.unwrap().unwrap();
        assert_eq!(configured.commands, vec!["cargo test"]);
        assert_eq!(configured.timeout_secs, 900);

        let mut expired = crate::models::Job::new(run.id, "expired".to_string());
        expired.timeout_secs = 5;
        JobQueries::create(&pool, &expired).await.unwrap();
        JobQueries::start(&pool, expired.id).await.unwrap();
        sqlx::query("UPDATE jobs SET started_at = ? WHERE id = ?")
            .bind((Utc::now() - chrono::Duration::seconds(60)).to_rfc3339())
            .bind(expired.id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();
        assert_eq!(JobQueries::reconcile_expired(&pool).await.unwrap(), 1);
        assert_eq!(
            JobQueries::get(&pool, expired.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "timed_out"
        );

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
        assert_eq!(jobs.len(), 2);

        sqlx::query("UPDATE jobs SET created_at = ? WHERE id = ?")
            .bind("2026-08-29 03:40:39")
            .bind(job.id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();
        assert!(JobQueries::get(&pool, job.id).await.is_err());
        assert!(JobQueries::list_by_run(&pool, run.id).await.is_err());
    }

    #[tokio::test]
    async fn test_reconcile_evidence_rows_grades_stranded_rows() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let user = crate::models::User::new(
            "owner".to_string(),
            "owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();
        let repo = crate::models::Repository::new(
            "evidence-repo".to_string(),
            user.id,
            "/git/evidence-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();
        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "Evidence Pipeline".to_string(),
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

        // Simulate the F21/F23 one-shot-write-loss shape: the completion
        // receipt (finished_at + result_json) persisted while the status and
        // started-at writes were lost, stranding the rows non-terminal. New
        // rows default to `pending`, and the lease write can fail from
        // `assigned` too, so both shapes must be covered.
        let mut succeeded_receipt =
            crate::models::Job::new(run.id, "receipt-succeeded".to_string());
        let mut unknown_receipt = crate::models::Job::new(run.id, "receipt-unknown".to_string());
        let mut garbage_receipt = crate::models::Job::new(run.id, "receipt-garbage".to_string());
        let mut live_queued = crate::models::Job::new(run.id, "still-queued".to_string());
        for job in [
            &mut succeeded_receipt,
            &mut unknown_receipt,
            &mut garbage_receipt,
            &mut live_queued,
        ] {
            JobQueries::create(&pool, job).await.unwrap();
        }
        let runner = crate::models::Runner::new(
            "evidence-runner".to_string(),
            crate::models::RunnerType::Docker,
            1,
        );
        RunnerQueries::create(&pool, &runner).await.unwrap();
        let assigned_receipt = crate::models::Job::new(run.id, "receipt-assigned".to_string());
        JobQueries::create(&pool, &assigned_receipt).await.unwrap();
        JobQueries::assign(&pool, assigned_receipt.id, runner.id)
            .await
            .unwrap();

        let tear = |job_id: gitforge_common::JobId, receipt: &str| {
            let pool = &pool;
            let job_id = job_id.to_string();
            let receipt = receipt.to_string();
            async move {
                sqlx::query("UPDATE jobs SET finished_at = ?, result_json = ? WHERE id = ?")
                    .bind(Utc::now().to_rfc3339())
                    .bind(receipt)
                    .bind(job_id)
                    .execute(pool.pool())
                    .await
                    .unwrap();
            }
        };
        tear(
            succeeded_receipt.id,
            r#"{"status":"succeeded","exit_code":0}"#,
        )
        .await;
        tear(unknown_receipt.id, r#"{"status":"exploded"}"#).await;
        tear(garbage_receipt.id, "runner died mid-write").await;
        tear(assigned_receipt.id, r#"{"status":"failed"}"#).await;

        assert_eq!(JobQueries::reconcile_evidence_rows(&pool).await.unwrap(), 4);

        let graded = |job_id: gitforge_common::JobId| {
            let pool = &pool;
            async move { JobQueries::get(pool, job_id).await.unwrap().unwrap() }
        };
        assert_eq!(graded(succeeded_receipt.id).await.status, "succeeded");
        // Unrecognized and unparseable evidence fail closed.
        assert_eq!(graded(unknown_receipt.id).await.status, "failed");
        assert_eq!(graded(garbage_receipt.id).await.status, "failed");
        assert_eq!(graded(assigned_receipt.id).await.status, "failed");

        // The repair grades from evidence without rewriting it: the receipt
        // columns keep their values and no start time is invented.
        let repaired = graded(succeeded_receipt.id).await;
        assert_eq!(
            repaired.result_json.as_deref(),
            Some(r#"{"status":"succeeded","exit_code":0}"#)
        );
        assert!(repaired.finished_at.is_some());
        assert!(repaired.started_at.is_none());

        // A live pre-terminal row without terminal evidence is left alone,
        // and a second sweep is a no-op (the repair is idempotent).
        assert_eq!(graded(live_queued.id).await.status, "pending");
        assert_eq!(JobQueries::reconcile_evidence_rows(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_job_queries_list_dispatchable() {
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

        // A planned row (durable DAG planning) sits at `pending` until its
        // dependency stage is released.
        let planned = crate::models::Job::new(run.id, "planned-later".to_string());
        JobQueries::create(&pool, &planned).await.unwrap();

        // A released row is flipped to `queued` before it is enqueued.
        let released = crate::models::Job::new(run.id, "build".to_string());
        JobQueries::create(&pool, &released).await.unwrap();
        JobQueries::update_status(&pool, released.id, "queued")
            .await
            .unwrap();

        // Only the queued row is dispatchable; the planned row must never
        // reach the scheduler's recovery scan.
        let dispatchable = JobQueries::list_dispatchable(&pool).await.unwrap();
        assert_eq!(dispatchable.len(), 1);
        assert_eq!(dispatchable[0].name, "build");
    }

    #[tokio::test]
    async fn test_dispatch_observability_reads() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let user = crate::models::User::new(
            "obs-owner".to_string(),
            "obs-owner@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();

        let repo = crate::models::Repository::new(
            "obs-repo".to_string(),
            user.id,
            "/git/obs-repo".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();

        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "Obs Pipeline".to_string(),
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

        // No started rows yet: the latency sample is honestly empty.
        assert!(JobQueries::recent_dispatch_latencies(&pool, 20)
            .await
            .unwrap()
            .is_empty());

        // One started job: the queued→started latency sample must be a
        // non-negative integer number of seconds, per job.
        let started = crate::models::Job::new(run.id, "started-job".to_string());
        JobQueries::create(&pool, &started).await.unwrap();
        JobQueries::start(&pool, started.id).await.unwrap();
        let latencies = JobQueries::recent_dispatch_latencies(&pool, 20)
            .await
            .unwrap();
        assert_eq!(latencies.len(), 1);
        assert_eq!(latencies[0].0, started.id);
        assert!(latencies[0].1 >= 0, "latency must not be negative");

        // The limit is honored (bounded report, newest first).
        let second = crate::models::Job::new(run.id, "second-started".to_string());
        JobQueries::create(&pool, &second).await.unwrap();
        JobQueries::start(&pool, second.id).await.unwrap();
        let bounded = JobQueries::recent_dispatch_latencies(&pool, 1)
            .await
            .unwrap();
        assert_eq!(bounded.len(), 1);
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

        // A runner-loss race can leave a durable row queued while retaining
        // the offline runner identity. Requeue must clear that identity too,
        // otherwise the replacement runner cannot claim the job.
        let stale_runner_id = RunnerId::new();
        let mut stale_runner = crate::models::Runner::new(
            "stale-requeue-runner".to_string(),
            crate::models::RunnerType::Docker,
            1,
        );
        stale_runner.id = stale_runner_id;
        RunnerQueries::create(&pool, &stale_runner).await.unwrap();
        JobQueries::assign(&pool, job.id, stale_runner_id)
            .await
            .unwrap();
        JobQueries::update_status(&pool, job.id, "queued")
            .await
            .unwrap();

        // Runner-loss recovery must clear an already-running lease, not only
        // jobs that were assigned but had not started execution yet.
        JobQueries::requeue(&pool, job.id).await.unwrap();
        let requeued = JobQueries::get(&pool, job.id).await.unwrap().unwrap();
        assert_eq!(requeued.status, "queued");
        assert!(requeued.runner_id.is_none());

        JobQueries::start(&pool, job.id).await.unwrap();

        let queued_job = crate::models::Job::new(run.id, "queued-stale".to_string());
        JobQueries::create(&pool, &queued_job).await.unwrap();
        JobQueries::assign(&pool, queued_job.id, stale_runner_id)
            .await
            .unwrap();
        JobQueries::update_status(&pool, queued_job.id, "queued")
            .await
            .unwrap();

        // Since #243, restart recovery fences a running job only when its
        // liveness proof is silent beyond the grace. A freshly started job
        // (recent `started_at` as the fallback proof) survives so its runner
        // can complete it against the durable lease.
        let survivor = crate::models::Job::new(run.id, "restart-survivor".to_string());
        JobQueries::create(&pool, &survivor).await.unwrap();
        JobQueries::start(&pool, survivor.id).await.unwrap();

        // A running job silent past the grace window is still fenced.
        let fenced = crate::models::Job::new(run.id, "restart-fenced".to_string());
        JobQueries::create(&pool, &fenced).await.unwrap();
        JobQueries::start(&pool, fenced.id).await.unwrap();
        let stale_started = (Utc::now() - chrono::Duration::seconds(600)).to_rfc3339();
        sqlx::query("UPDATE jobs SET started_at = ? WHERE id = ?")
            .bind(&stale_started)
            .bind(fenced.id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        assert_eq!(JobQueries::requeue_inflight(&pool, 300).await.unwrap(), 2);

        // The silent job is failed with the restart-fence receipt.
        let fenced_recovered = JobQueries::get(&pool, fenced.id).await.unwrap().unwrap();
        assert_eq!(fenced_recovered.status, "failed");
        assert!(fenced_recovered.runner_id.is_none());
        assert!(fenced_recovered.started_at.is_some());
        assert!(fenced_recovered
            .result_json
            .as_deref()
            .is_some_and(|receipt| receipt.contains("scheduler_restart_fenced_running_job")));
        // The fresh job survives, still running and reclaimable.
        let survivor_recovered = JobQueries::get(&pool, survivor.id).await.unwrap().unwrap();
        assert_eq!(survivor_recovered.status, "running");
        let queued_recovered = JobQueries::get(&pool, queued_job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(queued_recovered.status, "queued");
        assert!(queued_recovered.runner_id.is_none());
    }

    #[tokio::test]
    async fn test_job_heartbeat_is_lease_gated_and_recovers_runner() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let user = crate::models::User::new(
            "beat-owner".to_string(),
            "beat@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();
        let repo = crate::models::Repository::new(
            "beat-repo".to_string(),
            user.id,
            "/git/beat".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();
        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "beat-pipeline".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: Utc::now(),
        };
        PipelineQueries::create(&pool, &pipeline).await.unwrap();
        let run = crate::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "main".to_string(),
            "abc123".to_string(),
        );
        PipelineRunQueries::create(&pool, &run).await.unwrap();

        let runner_id = RunnerId::new();
        let mut runner = crate::models::Runner::new(
            "beat-runner".to_string(),
            crate::models::RunnerType::Docker,
            1,
        );
        runner.id = runner_id;
        RunnerQueries::create(&pool, &runner).await.unwrap();
        let job = crate::models::Job::new(run.id, "beat".to_string());
        JobQueries::create(&pool, &job).await.unwrap();
        // Lease sync parks the row at `assigned`; the row must be
        // dispatchable first.
        JobQueries::update_status(&pool, job.id, "queued")
            .await
            .unwrap();
        assert!(JobQueries::sync_lease(&pool, job.id, runner_id, "lease-1")
            .await
            .unwrap());

        // The runner's global state is stale and offline: a delivered job
        // beat must refresh it and recover the runner, because the request
        // proves the runner process is alive and in contact.
        let stale_beat = (Utc::now() - chrono::Duration::seconds(600)).to_rfc3339();
        sqlx::query("UPDATE runners SET last_heartbeat = ?, status = 'offline' WHERE id = ?")
            .bind(&stale_beat)
            .bind(runner_id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();

        // A wrong lease is rejected and writes nothing.
        assert!(!JobQueries::heartbeat(&pool, job.id, runner_id, "lease-2")
            .await
            .unwrap());
        let untouched = JobQueries::get(&pool, job.id).await.unwrap().unwrap();
        assert!(untouched.heartbeat_at.is_none());
        let still_offline = RunnerQueries::get(&pool, runner_id).await.unwrap().unwrap();
        assert_eq!(still_offline.status, "offline");

        // The matching lease is accepted and recovers both sides.
        assert!(JobQueries::heartbeat(&pool, job.id, runner_id, "lease-1")
            .await
            .unwrap());
        let beaten = JobQueries::get(&pool, job.id).await.unwrap().unwrap();
        assert!(beaten.heartbeat_at.is_some());
        let recovered = RunnerQueries::get(&pool, runner_id).await.unwrap().unwrap();
        assert_eq!(recovered.status, "online");
        let stale_time = DateTime::parse_from_rfc3339(&stale_beat)
            .unwrap()
            .with_timezone(&Utc);
        assert_ne!(recovered.last_heartbeat, Some(stale_time));

        // A terminal job can no longer be heartbeated.
        JobQueries::update_status(&pool, job.id, "failed")
            .await
            .unwrap();
        assert!(!JobQueries::heartbeat(&pool, job.id, runner_id, "lease-1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_requeue_inflight_honors_job_heartbeat_over_runner_heartbeat() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let user = crate::models::User::new(
            "grace-owner".to_string(),
            "grace@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();
        let repo = crate::models::Repository::new(
            "grace-repo".to_string(),
            user.id,
            "/git/grace".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();
        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "grace-pipeline".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: Utc::now(),
        };
        PipelineQueries::create(&pool, &pipeline).await.unwrap();
        let run = crate::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "main".to_string(),
            "abc123".to_string(),
        );
        PipelineRunQueries::create(&pool, &run).await.unwrap();

        // Two runners, both globally stale: the fence must judge each job by
        // its own liveness proof, not by a runner-wide verdict.
        let stale_runner_id = RunnerId::new();
        let mut stale_runner = crate::models::Runner::new(
            "grace-stale-runner".to_string(),
            crate::models::RunnerType::Docker,
            1,
        );
        stale_runner.id = stale_runner_id;
        RunnerQueries::create(&pool, &stale_runner).await.unwrap();
        let beat_runner_id = RunnerId::new();
        let mut beat_runner = crate::models::Runner::new(
            "grace-beat-runner".to_string(),
            crate::models::RunnerType::Docker,
            1,
        );
        beat_runner.id = beat_runner_id;
        RunnerQueries::create(&pool, &beat_runner).await.unwrap();
        let stale_beat = (Utc::now() - chrono::Duration::seconds(600)).to_rfc3339();
        for runner_id in [stale_runner_id, beat_runner_id] {
            sqlx::query("UPDATE runners SET last_heartbeat = ? WHERE id = ?")
                .bind(&stale_beat)
                .bind(runner_id.to_string())
                .execute(pool.pool())
                .await
                .unwrap();
        }

        // Legacy fallback: no per-job heartbeat at all, and the runner's
        // global heartbeat is stale past the grace — the job is fenced.
        let legacy = crate::models::Job::new(run.id, "legacy-stale".to_string());
        JobQueries::create(&pool, &legacy).await.unwrap();
        JobQueries::update_status(&pool, legacy.id, "queued")
            .await
            .unwrap();
        assert!(
            JobQueries::sync_lease(&pool, legacy.id, stale_runner_id, "lease-legacy")
                .await
                .unwrap()
        );
        JobQueries::start(&pool, legacy.id).await.unwrap();

        // The #243 case: the runner's global heartbeat is equally stale, but
        // the job itself reported liveness moments ago — it survives.
        let alive = crate::models::Job::new(run.id, "job-beat-fresh".to_string());
        JobQueries::create(&pool, &alive).await.unwrap();
        JobQueries::update_status(&pool, alive.id, "queued")
            .await
            .unwrap();
        assert!(
            JobQueries::sync_lease(&pool, alive.id, beat_runner_id, "lease-alive")
                .await
                .unwrap()
        );
        JobQueries::start(&pool, alive.id).await.unwrap();
        assert!(
            JobQueries::heartbeat(&pool, alive.id, beat_runner_id, "lease-alive")
                .await
                .unwrap()
        );

        assert_eq!(JobQueries::requeue_inflight(&pool, 300).await.unwrap(), 1);
        let legacy_recovered = JobQueries::get(&pool, legacy.id).await.unwrap().unwrap();
        assert_eq!(legacy_recovered.status, "failed");
        assert!(legacy_recovered
            .result_json
            .as_deref()
            .is_some_and(|receipt| receipt.contains("scheduler_restart_fenced_running_job")));
        let alive_recovered = JobQueries::get(&pool, alive.id).await.unwrap().unwrap();
        assert_eq!(alive_recovered.status, "running");
        assert!(alive_recovered.lease_token.is_some());
    }

    #[tokio::test]
    async fn test_list_running_returns_only_running_jobs() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let user = crate::models::User::new(
            "running-owner".to_string(),
            "running@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.unwrap();
        let repo = crate::models::Repository::new(
            "running-repo".to_string(),
            user.id,
            "/git/running".to_string(),
        );
        RepoQueries::create(&pool, &repo).await.unwrap();
        let pipeline = crate::models::Pipeline {
            id: PipelineId::new(),
            repo_id: repo.id,
            name: "running-pipeline".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: Utc::now(),
        };
        PipelineQueries::create(&pool, &pipeline).await.unwrap();
        let run = crate::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "main".to_string(),
            "abc123".to_string(),
        );
        PipelineRunQueries::create(&pool, &run).await.unwrap();

        let running = crate::models::Job::new(run.id, "running".to_string());
        JobQueries::create(&pool, &running).await.unwrap();
        JobQueries::start(&pool, running.id).await.unwrap();
        let queued = crate::models::Job::new(run.id, "queued".to_string());
        JobQueries::create(&pool, &queued).await.unwrap();
        JobQueries::update_status(&pool, queued.id, "queued")
            .await
            .unwrap();
        let failed = crate::models::Job::new(run.id, "failed".to_string());
        JobQueries::create(&pool, &failed).await.unwrap();
        JobQueries::start(&pool, failed.id).await.unwrap();
        JobQueries::update_status(&pool, failed.id, "failed")
            .await
            .unwrap();

        let listed = JobQueries::list_running(&pool).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, running.id);
        assert_eq!(listed[0].status, "running");
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

        sqlx::query("UPDATE runners SET created_at = ? WHERE id = ?")
            .bind("2026-08-29 03:40:39")
            .bind(runner.id.to_string())
            .execute(pool.pool())
            .await
            .unwrap();
        assert!(RunnerQueries::get(&pool, runner.id).await.is_err());
        assert!(RunnerQueries::list_online(&pool).await.is_err());
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

        sqlx::query("UPDATE events SET payload = ? WHERE id = (SELECT id FROM events LIMIT 1)")
            .bind("not-json")
            .execute(pool.pool())
            .await
            .unwrap();
        assert!(EventQueries::list_recent(&pool, 5).await.is_err());
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

    #[tokio::test]
    async fn test_refresh_token_lifecycle_filters_and_prunes() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();

        let user = crate::models::User::new(
            "refresh-lifecycle".to_string(),
            "refresh-lifecycle@example.com".to_string(),
            "hash".to_string(),
        );
        crate::queries::UserQueries::create(&pool, &user)
            .await
            .unwrap();

        let live_digest = "digest-live";
        let expired_digest = "digest-expired";
        let now = Utc::now();
        RefreshTokenQueries::create(
            &pool,
            user.id,
            live_digest,
            now + chrono::Duration::days(30),
        )
        .await
        .unwrap();
        RefreshTokenQueries::create(
            &pool,
            user.id,
            expired_digest,
            now - chrono::Duration::days(1),
        )
        .await
        .unwrap();

        // Only the unexpired row is live.
        let live = RefreshTokenQueries::find_active_by_hash(&pool, live_digest)
            .await
            .unwrap()
            .expect("unexpired row must be live");
        assert_eq!(live.user_id, user.id);
        assert!(
            RefreshTokenQueries::find_active_by_hash(&pool, expired_digest)
                .await
                .unwrap()
                .is_none()
        );

        // Revocation is immediate and idempotent.
        RefreshTokenQueries::revoke(&pool, live_digest)
            .await
            .unwrap();
        assert!(RefreshTokenQueries::find_active_by_hash(&pool, live_digest)
            .await
            .unwrap()
            .is_none());
        RefreshTokenQueries::revoke(&pool, live_digest)
            .await
            .unwrap();

        // revoke_all_for_user only touches that user's rows.
        let other = crate::models::User::new(
            "refresh-lifecycle-2".to_string(),
            "refresh-lifecycle-2@example.com".to_string(),
            "hash".to_string(),
        );
        crate::queries::UserQueries::create(&pool, &other)
            .await
            .unwrap();
        RefreshTokenQueries::create(
            &pool,
            other.id,
            "digest-other",
            now + chrono::Duration::days(30),
        )
        .await
        .unwrap();
        let revoked = RefreshTokenQueries::revoke_all_for_user(&pool, user.id)
            .await
            .unwrap();
        // Only the expired-but-never-revoked row counts: the live row was
        // revoked earlier in this test and revocation is not re-applied.
        assert_eq!(revoked, 1);
        assert!(
            RefreshTokenQueries::find_active_by_hash(&pool, "digest-other")
                .await
                .unwrap()
                .is_some()
        );

        // Prune is age-based housekeeping: a cutoff of `now` drops only
        // the row already expired before it; rows revoked after `now`
        // (and the other user's future-expiry row) survive.
        let pruned = RefreshTokenQueries::prune(&pool, now).await.unwrap();
        assert_eq!(pruned, 1);

        // A cutoff past every lifetime clears the table.
        let pruned = RefreshTokenQueries::prune(&pool, now + chrono::Duration::days(31))
            .await
            .unwrap();
        assert_eq!(pruned, 2);
        assert!(
            RefreshTokenQueries::find_active_by_hash(&pool, "digest-other")
                .await
                .unwrap()
                .is_none()
        );
    }
}
