//! Durable, idempotent publication work items.

use crate::Pool;
use chrono::{DateTime, Utc};
use gitforge_common::{Error, JobId, Result};
use sqlx::Row;
use uuid::Uuid;

const PENDING: &str = "pending";
const IN_FLIGHT: &str = "in_flight";
const PUBLISHED: &str = "published";
const RETRYABLE: &str = "retryable";
const PERMANENT_FAILURE: &str = "permanent_failure";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationOutboxItem {
    pub id: Uuid,
    pub job_id: JobId,
    pub provider: String,
    pub kind: String,
    pub payload: String,
    pub state: String,
    pub attempts: i64,
    pub next_attempt_at: DateTime<Utc>,
    pub claim_token: Option<String>,
    pub claim_until: Option<DateTime<Utc>>,
    pub external_id: Option<String>,
    pub last_error: Option<String>,
}

fn parse_time(value: String, field: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| Error::database(format!("invalid {}: {}", field, error)))
}

fn hydrate(row: &sqlx::sqlite::SqliteRow) -> Result<PublicationOutboxItem> {
    let id = Uuid::parse_str(
        &row.try_get::<String, _>("id")
            .map_err(|error| Error::database(format!("invalid publication ID: {}", error)))?,
    )
    .map_err(|error| Error::database(format!("invalid publication ID: {}", error)))?;
    let job_id =
        JobId::from(
            Uuid::parse_str(&row.try_get::<String, _>("job_id").map_err(|error| {
                Error::database(format!("invalid publication job ID: {}", error))
            })?)
            .map_err(|error| Error::database(format!("invalid publication job ID: {}", error)))?,
        );
    let next_attempt_at = parse_time(
        row.try_get("next_attempt_at")
            .map_err(|error| Error::database(format!("invalid next attempt time: {}", error)))?,
        "next attempt time",
    )?;
    let claim_until = row
        .try_get::<Option<String>, _>("claim_until")
        .map_err(|error| Error::database(format!("invalid claim time: {}", error)))?
        .map(|value| parse_time(value, "claim time"))
        .transpose()?;
    Ok(PublicationOutboxItem {
        id,
        job_id,
        provider: row
            .try_get("provider")
            .map_err(|e| Error::database(e.to_string()))?,
        kind: row
            .try_get("kind")
            .map_err(|e| Error::database(e.to_string()))?,
        payload: row
            .try_get("payload")
            .map_err(|e| Error::database(e.to_string()))?,
        state: row
            .try_get("state")
            .map_err(|e| Error::database(e.to_string()))?,
        attempts: row
            .try_get("attempts")
            .map_err(|e| Error::database(e.to_string()))?,
        next_attempt_at,
        claim_token: row
            .try_get("claim_token")
            .map_err(|e| Error::database(e.to_string()))?,
        claim_until,
        external_id: row
            .try_get("external_id")
            .map_err(|e| Error::database(e.to_string()))?,
        last_error: row
            .try_get("last_error")
            .map_err(|e| Error::database(e.to_string()))?,
    })
}

pub struct PublicationOutboxQueries;

impl PublicationOutboxQueries {
    /// Insert one logical publication. Repeated calls return the original row.
    pub async fn enqueue(
        pool: &Pool,
        job_id: JobId,
        provider: &str,
        kind: &str,
        payload: &str,
    ) -> Result<PublicationOutboxItem> {
        if provider.is_empty() || kind.is_empty() || payload.is_empty() {
            return Err(Error::database("publication fields must not be empty"));
        }
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO publication_outbox
             (id, job_id, provider, kind, payload, state, attempts, next_attempt_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(job_id.to_string())
        .bind(provider)
        .bind(kind)
        .bind(payload)
        .bind(PENDING)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .execute(pool.pool())
        .await
        .map_err(|error| Error::database(format!("failed to enqueue publication: {}", error)))?;
        Self::get(pool, job_id, provider, kind)
            .await?
            .ok_or_else(|| Error::database("publication disappeared immediately after enqueue"))
    }

    pub async fn get(
        pool: &Pool,
        job_id: JobId,
        provider: &str,
        kind: &str,
    ) -> Result<Option<PublicationOutboxItem>> {
        let row = sqlx::query(
            "SELECT * FROM publication_outbox WHERE job_id = ? AND provider = ? AND kind = ?",
        )
        .bind(job_id.to_string())
        .bind(provider)
        .bind(kind)
        .fetch_optional(pool.pool())
        .await
        .map_err(|error| Error::database(format!("failed to read publication: {}", error)))?;
        row.as_ref().map(hydrate).transpose()
    }

    /// Claim the oldest due item. The conditional update prevents two workers
    /// from owning the same item when they race.
    pub async fn claim_due(
        pool: &Pool,
        now: DateTime<Utc>,
        lease: Duration,
    ) -> Result<Option<PublicationOutboxItem>> {
        let now_text = now.to_rfc3339();
        let row = sqlx::query(
            "SELECT id FROM publication_outbox
             WHERE (state IN (?, ?) AND next_attempt_at <= ?)
                OR (state = ? AND claim_until <= ?)
             ORDER BY next_attempt_at, created_at LIMIT 1",
        )
        .bind(PENDING)
        .bind(RETRYABLE)
        .bind(&now_text)
        .bind(IN_FLIGHT)
        .bind(&now_text)
        .fetch_optional(pool.pool())
        .await
        .map_err(|error| Error::database(format!("failed to find due publication: {}", error)))?;
        let Some(row) = row else { return Ok(None) };
        let id: String = row
            .try_get("id")
            .map_err(|error| Error::database(format!("invalid publication ID: {}", error)))?;
        let token = Uuid::new_v4().to_string();
        let until = (now + lease).to_rfc3339();
        let changed = sqlx::query(
            "UPDATE publication_outbox SET state = ?, claim_token = ?, claim_until = ?,
             attempts = attempts + 1, updated_at = ?
             WHERE id = ? AND ((state IN (?, ?) AND next_attempt_at <= ?)
                OR (state = ? AND claim_until <= ?))",
        )
        .bind(IN_FLIGHT)
        .bind(&token)
        .bind(&until)
        .bind(&now_text)
        .bind(&id)
        .bind(PENDING)
        .bind(RETRYABLE)
        .bind(&now_text)
        .bind(IN_FLIGHT)
        .bind(&now_text)
        .execute(pool.pool())
        .await
        .map_err(|error| Error::database(format!("failed to claim publication: {}", error)))?;
        if changed.rows_affected() != 1 {
            return Ok(None);
        }
        let result = sqlx::query("SELECT * FROM publication_outbox WHERE id = ?")
            .bind(id)
            .fetch_one(pool.pool())
            .await
            .map_err(|error| {
                Error::database(format!("failed to load claimed publication: {}", error))
            })?;
        hydrate(&result).map(Some)
    }

    pub async fn mark_published(
        pool: &Pool,
        id: Uuid,
        claim_token: &str,
        external_id: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE publication_outbox SET state = ?, external_id = ?, claim_token = NULL,
             claim_until = NULL, last_error = NULL, updated_at = ?
             WHERE id = ? AND state = ? AND claim_token = ?",
        )
        .bind(PUBLISHED)
        .bind(external_id)
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .bind(IN_FLIGHT)
        .bind(claim_token)
        .execute(pool.pool())
        .await
        .map_err(|error| Error::database(format!("failed to publish publication: {}", error)))?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn schedule_retry(
        pool: &Pool,
        id: Uuid,
        claim_token: &str,
        next_attempt_at: DateTime<Utc>,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE publication_outbox SET state = ?, next_attempt_at = ?, claim_token = NULL,
             claim_until = NULL, last_error = ?, updated_at = ?
             WHERE id = ? AND state = ? AND claim_token = ?",
        )
        .bind(RETRYABLE)
        .bind(next_attempt_at.to_rfc3339())
        .bind(error)
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .bind(IN_FLIGHT)
        .bind(claim_token)
        .execute(pool.pool())
        .await
        .map_err(|error| {
            Error::database(format!("failed to schedule publication retry: {}", error))
        })?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_permanent_failure(
        pool: &Pool,
        id: Uuid,
        claim_token: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE publication_outbox SET state = ?, claim_token = NULL, claim_until = NULL,
             last_error = ?, updated_at = ? WHERE id = ? AND state = ? AND claim_token = ?",
        )
        .bind(PERMANENT_FAILURE)
        .bind(error)
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .bind(IN_FLIGHT)
        .bind(claim_token)
        .execute(pool.pool())
        .await
        .map_err(|error| {
            Error::database(format!("failed to mark publication failure: {}", error))
        })?;
        Ok(result.rows_affected() == 1)
    }
}

pub use std::time::Duration;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn enqueue_is_idempotent_and_claim_can_be_acknowledged() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let job_id = JobId::new();
        let first = PublicationOutboxQueries::enqueue(&pool, job_id, "github", "check_run", "{}")
            .await
            .unwrap();
        let second = PublicationOutboxQueries::enqueue(&pool, job_id, "github", "check_run", "{}")
            .await
            .unwrap();
        assert_eq!(first.id, second.id);
        let claimed =
            PublicationOutboxQueries::claim_due(&pool, Utc::now(), Duration::from_secs(30))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(claimed.state, IN_FLIGHT);
        assert_eq!(claimed.attempts, 1);
        assert!(PublicationOutboxQueries::mark_published(
            &pool,
            claimed.id,
            claimed.claim_token.as_deref().unwrap(),
            "123",
            Utc::now(),
        )
        .await
        .unwrap());
        assert_eq!(
            PublicationOutboxQueries::get(&pool, job_id, "github", "check_run")
                .await
                .unwrap()
                .unwrap()
                .state,
            PUBLISHED
        );
    }

    #[tokio::test]
    async fn retry_requires_active_claim_and_becomes_due() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let item = PublicationOutboxQueries::enqueue(
            &pool,
            JobId::new(),
            "github",
            "check_run",
            "{\"ok\":true}",
        )
        .await
        .unwrap();
        assert!(!PublicationOutboxQueries::schedule_retry(
            &pool,
            item.id,
            "wrong-token",
            Utc::now(),
            "rate limited",
            Utc::now(),
        )
        .await
        .unwrap());
        let claimed =
            PublicationOutboxQueries::claim_due(&pool, Utc::now(), Duration::from_secs(1))
                .await
                .unwrap()
                .unwrap();
        assert!(PublicationOutboxQueries::schedule_retry(
            &pool,
            claimed.id,
            claimed.claim_token.as_deref().unwrap(),
            Utc::now(),
            "rate limited",
            Utc::now(),
        )
        .await
        .unwrap());
        assert_eq!(
            PublicationOutboxQueries::claim_due(&pool, Utc::now(), Duration::from_secs(1))
                .await
                .unwrap()
                .unwrap()
                .attempts,
            2
        );
    }
}
