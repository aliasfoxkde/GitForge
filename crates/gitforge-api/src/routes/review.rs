//! Code review run API routes
//!
//! Submits and reads persisted review runs (ADR 20260905 contract). This
//! slice only persists runs and exposes read access; provider execution and
//! worker dispatch are intentionally out of scope. The dispatch seam is the
//! `pending` run row created here: a future worker claims pending runs and
//! advances them through [`gitforge_review::domain::ReviewRunState`]
//! transitions via `gitforge_db::queries::ReviewQueries::transition_run`,
//! the same durable control-plane pattern the CI scheduler uses.

use crate::auth::Claims;
use axum::{
    extract::{Extension, Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use gitforge_db::{
    queries::{RepoQueries, ReviewQueries},
    Pool,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

/// Maximum accepted length for base/head SHA strings (SHA-256 hex is 64).
const MAX_SHA_LEN: usize = 128;
/// Maximum accepted idempotency key length; mirrors the database CHECK.
const MAX_IDEMPOTENCY_KEY_LEN: usize = 128;
/// Maximum accepted attempt value.
const MAX_ATTEMPT: i64 = 10_000;
/// Default and maximum page sizes for finding listings.
const FINDINGS_DEFAULT_LIMIT: usize = 100;
const FINDINGS_MAX_LIMIT: usize = 500;

type ApiResult<T> = Result<T, Box<Response>>;

/// Review run submission request. Exactly one form of repository identity is
/// accepted: `repo_id`, or `owner` plus `name`.
#[derive(Debug, Deserialize, Serialize)]
pub struct SubmitReviewRunRequest {
    /// Repository UUID (mutually exclusive with `owner`/`name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
    /// Repository owner username (used with `name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Repository name (used with `owner`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub base_sha: String,
    pub head_sha: String,
    pub idempotency_key: String,
    pub attempt: i64,
}

/// Bounded listing controls for review findings.
#[derive(Debug, Deserialize)]
pub struct ListFindingsQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

/// Review routes
pub fn review_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/review-runs", post(submit_review_run))
        .route("/review-runs/{id}", get(get_review_run))
        .route("/review-runs/{id}/findings", get(list_review_findings))
}

fn can_manage_reviews(claims: &Claims) -> bool {
    matches!(claims.role.as_str(), "admin" | "maintainer")
}

/// Require ownership of a repository, with admin/maintainer override.
/// `Ok(true)` means authorized, `Ok(false)` means the repository exists but
/// belongs to someone else, `Err` is a transport failure.
async fn authorize_repo(pool: &Pool, claims: &Claims, repo_id: Option<Uuid>) -> ApiResult<bool> {
    if can_manage_reviews(claims) {
        return Ok(true);
    }
    let Some(repo_id) = repo_id else {
        // Runs without a resolvable repository are only visible to
        // administrators so authorization boundaries cannot be bypassed.
        return Ok(false);
    };
    let repo_id = gitforge_common::RepoId::from(repo_id);
    match RepoQueries::get(pool, repo_id).await {
        Ok(Some(repo)) => Ok(repo.owner_id == claims.user_id),
        Ok(None) => Ok(false),
        Err(error) => {
            tracing::error!(%error, %repo_id, "failed to authorize repository access");
            Err(Box::new(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Repository authorization is temporarily unavailable",
            )))
        }
    }
}

fn error_response(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({"error": error, "message": message}))).into_response()
}

fn not_found(error: &str, message: &str) -> Response {
    error_response(StatusCode::NOT_FOUND, error, message)
}

/// Serialize a persisted review run for API responses.
fn run_json(run: &gitforge_db::queries::ReviewRun) -> serde_json::Value {
    json!({
        "id": run.id.to_string(),
        "repo_id": run.repo_id.map(|id| id.to_string()),
        "base_sha": run.base_sha,
        "head_sha": run.head_sha,
        "idempotency_key": run.idempotency_key,
        "status": run.status.to_string(),
        "attempt": run.attempt,
        "receipt_id": run.receipt_id,
        "created_at": run.created_at.to_rfc3339(),
        "updated_at": run.updated_at.to_rfc3339(),
    })
}

/// Serialize a persisted review finding for API responses.
fn finding_json(finding: &gitforge_db::queries::ReviewFinding) -> serde_json::Value {
    json!({
        "id": finding.id.to_string(),
        "run_id": finding.run_id.to_string(),
        "source": finding.source,
        "fingerprint": finding.fingerprint,
        "path": finding.path,
        "line": finding.line,
        "severity": finding.severity,
        "category": finding.category,
        "title": finding.title,
        "message": finding.message,
        "evidence": finding.evidence,
        "confidence": finding.confidence,
        "position_status": finding.position_status.to_string(),
        "disposition": finding.disposition,
        "created_at": finding.created_at.to_rfc3339(),
        "updated_at": finding.updated_at.to_rfc3339(),
    })
}

/// Validate the submitted field bounds before any repository lookup. SHA
/// strings must be non-empty and bounded; the idempotency key mirrors the
/// database CHECK constraint; attempts must be positive and bounded.
fn validate_submission(request: &SubmitReviewRunRequest) -> ApiResult<()> {
    let sha_invalid = |field: &str| {
        Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_review_submission",
            &format!("{} must be 1-{} characters", field, MAX_SHA_LEN),
        ))
    };
    if request.base_sha.is_empty() || request.base_sha.len() > MAX_SHA_LEN {
        return Err(sha_invalid("base_sha"));
    }
    if request.head_sha.is_empty() || request.head_sha.len() > MAX_SHA_LEN {
        return Err(sha_invalid("head_sha"));
    }
    if request.idempotency_key.is_empty() || request.idempotency_key.len() > MAX_IDEMPOTENCY_KEY_LEN
    {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_review_submission",
            &format!(
                "idempotency_key must be 1-{} characters",
                MAX_IDEMPOTENCY_KEY_LEN
            ),
        )));
    }
    if !(1..=MAX_ATTEMPT).contains(&request.attempt) {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_review_submission",
            &format!("attempt must be between 1 and {}", MAX_ATTEMPT),
        )));
    }
    let has_repo_id = request.repo_id.is_some();
    let has_owner_name = request.owner.is_some() || request.name.is_some();
    if has_repo_id == has_owner_name {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_review_submission",
            "exactly one of repo_id or owner+name must identify the repository",
        )));
    }
    if has_owner_name
        && (request.owner.as_deref().is_none_or(str::is_empty)
            || request.owner.as_deref().is_some_and(|o| o.len() > 128)
            || request.name.as_deref().is_none_or(str::is_empty)
            || request.name.as_deref().is_some_and(|n| n.len() > 128))
    {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_review_submission",
            "owner and name must be 1-128 characters",
        )));
    }
    Ok(())
}

/// Resolve the repository named by the submission, if it exists.
async fn resolve_repository(
    pool: &Pool,
    request: &SubmitReviewRunRequest,
) -> ApiResult<Option<gitforge_db::models::Repository>> {
    if let Some(repo_id) = &request.repo_id {
        let uuid = Uuid::parse_str(repo_id).map_err(|_| {
            Box::new(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_repo_id",
                "repo_id must be a UUID",
            ))
        })?;
        return RepoQueries::get(pool, gitforge_common::RepoId::from(uuid))
            .await
            .map_err(|error| {
                tracing::error!(%error, %repo_id, "failed to load repository for review submission");
                Box::new(error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database_error",
                    "Repository lookup failed",
                ))
            });
    }
    let owner = request.owner.as_deref().unwrap_or_default();
    let name = request.name.as_deref().unwrap_or_default();
    RepoQueries::get_by_owner_and_name(pool, owner, name)
        .await
        .map_err(|error| {
            tracing::error!(%error, %owner, %name, "failed to load repository for review submission");
            Box::new(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Repository lookup failed",
            ))
        })
}

/// Load a review run and enforce repository-scoped authorization. Missing
/// runs and runs the caller cannot see both yield `404` so private review
/// activity is not leaked across authorization boundaries.
async fn authorized_review_run(
    pool: &Pool,
    claims: &Claims,
    run_id: Uuid,
) -> ApiResult<gitforge_db::queries::ReviewRun> {
    let run = ReviewQueries::get_run(pool, run_id)
        .await
        .map_err(|error| {
            tracing::error!(%error, %run_id, "failed to load review run");
            Box::new(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Review run lookup failed",
            ))
        })?
        .ok_or_else(|| Box::new(not_found("review_run_not_found", "Review run not found")))?;

    match authorize_repo(pool, claims, run.repo_id).await? {
        true => Ok(run),
        false => Err(Box::new(not_found(
            "review_run_not_found",
            "Review run not found",
        ))),
    }
}

/// Submit a review run.
///
/// - `201` with the new run when the idempotency key is fresh.
/// - `200` with the existing run when the key matches the same head SHA.
/// - `409` when the key was already used against a different head SHA.
async fn submit_review_run(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
    Json(request): Json<SubmitReviewRunRequest>,
) -> Response {
    if let Err(response) = validate_submission(&request) {
        return *response;
    }
    let repo = match resolve_repository(&pool, &request).await {
        Ok(Some(repo)) => repo,
        Ok(None) => {
            return not_found("repository_not_found", "Repository not found");
        }
        Err(response) => return *response,
    };
    match authorize_repo(&pool, &claims, Some(repo.id.into())).await {
        Ok(true) => {}
        Ok(false) => {
            return error_response(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Review submission is not permitted for this repository",
            );
        }
        Err(response) => return *response,
    }

    let new_run = gitforge_db::queries::NewReviewRun {
        repo_id: Some(repo.id.into()),
        base_sha: request.base_sha.clone(),
        head_sha: request.head_sha.clone(),
        idempotency_key: request.idempotency_key.clone(),
        attempt: request.attempt,
    };
    match ReviewQueries::create_or_get_run(&pool, &new_run).await {
        Ok(gitforge_db::queries::CreateOrGetReviewRun::Created(run)) => (
            StatusCode::CREATED,
            Json(json!({"status": "created", "run": run_json(&run)})),
        )
            .into_response(),
        Ok(gitforge_db::queries::CreateOrGetReviewRun::Existing(run)) => (
            StatusCode::OK,
            Json(json!({"status": "already_exists", "run": run_json(&run)})),
        )
            .into_response(),
        Ok(gitforge_db::queries::CreateOrGetReviewRun::HeadConflict {
            existing,
            requested_head_sha,
        }) => {
            let message = format!(
                "idempotency key was already used for head {}; it cannot be reused for head {}",
                existing.head_sha, requested_head_sha
            );
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "idempotency_key_head_conflict",
                    "message": message,
                    "existing_run_id": existing.id.to_string(),
                })),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(%error, "failed to create review run");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Review run creation failed",
            )
        }
    }
}

/// Read a single review run by ID.
async fn get_review_run(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
) -> Response {
    let run_id = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_id",
                "Invalid review run ID format",
            );
        }
    };
    match authorized_review_run(&pool, &claims, run_id).await {
        Ok(run) => (StatusCode::OK, Json(run_json(&run))).into_response(),
        Err(response) => *response,
    }
}

/// List findings for a review run in deterministic order (path, then line
/// with NULL lines first, then fingerprint) and bounded output.
async fn list_review_findings(
    Extension(pool): Extension<Arc<Pool>>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<String>,
    Query(query): Query<ListFindingsQuery>,
) -> Response {
    let run_id = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_id",
                "Invalid review run ID format",
            );
        }
    };
    let limit = query.limit.unwrap_or(FINDINGS_DEFAULT_LIMIT);
    let offset = query.offset.unwrap_or(0);
    if limit == 0 || limit > FINDINGS_MAX_LIMIT {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_pagination",
            &format!("limit must be between 1 and {}", FINDINGS_MAX_LIMIT),
        );
    }
    let run = match authorized_review_run(&pool, &claims, run_id).await {
        Ok(run) => run,
        Err(response) => return *response,
    };
    let mut findings = match ReviewQueries::list_findings(&pool, run.id).await {
        Ok(findings) => findings,
        Err(error) => {
            tracing::error!(%error, run_id = %run.id, "failed to list review findings");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Review findings are temporarily unavailable",
            );
        }
    };
    // The query orders by path and line; the stable secondary sort on
    // fingerprint makes the total order fully deterministic for ties.
    findings.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    let total = findings.len();
    let page: Vec<serde_json::Value> = findings
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|finding| finding_json(&finding))
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "run_id": run.id.to_string(),
            "run_status": run.status.to_string(),
            "total": total,
            "limit": limit,
            "offset": offset,
            "findings": page,
        })),
    )
        .into_response()
}
