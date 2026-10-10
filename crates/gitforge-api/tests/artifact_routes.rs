//! Integration tests for the artifact route handlers.
//!
//! These drive the real router with an in-memory database and a
//! temp-dir `FileStorage`, covering the ownership authorization matrix
//! (owner / admin / unrelated developer) and the error contracts of
//! every artifact endpoint.

// Test-harness exemption, same discipline as the sibling suites:
// `allow-unwrap-in-tests` covers `#[test]` bodies, but boot/seed/fixture
// helper functions in a test target are neither `#[test]` fns nor
// `#[cfg(test)]`, a class the clippy.toml config cannot address. Setup
// failing IS the assertion -- a panic aborts the run loudly. Production
// code keeps the denies.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use gitforge_api::{ApiAuth, ApiServer};
use gitforge_common::JobId;
use gitforge_db::{
    models::{Job, Pipeline, PipelineRun, Repository, User},
    queries::{JobQueries, PipelineQueries, PipelineRunQueries, RepoQueries, UserQueries},
    Pool,
};
use gitforge_storage::{Artifact, ArtifactId, ArtifactStore, FileStorage};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct Fixture {
    app: Router,
    storage: Arc<FileStorage>,
    // Keeps the storage root alive for the test's lifetime.
    _artifact_root: tempfile::TempDir,
    owner_token: String,
    admin_token: String,
    intruder_token: String,
    job_id: JobId,
    #[allow(dead_code)]
    other_job_id: JobId,
}

async fn seed() -> Fixture {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let owner = User::new(
        "artifact-owner".to_string(),
        "artifact-owner@example.com".to_string(),
        "hash".to_string(),
    );
    let admin = User::new(
        "artifact-admin".to_string(),
        "artifact-admin@example.com".to_string(),
        "hash".to_string(),
    );
    let intruder = User::new(
        "artifact-intruder".to_string(),
        "artifact-intruder@example.com".to_string(),
        "hash".to_string(),
    );
    for user in [&owner, &admin, &intruder] {
        UserQueries::create(&pool, user).await.unwrap();
    }
    assert!(UserQueries::set_role(&pool, admin.id, "admin")
        .await
        .unwrap());

    let repo = Repository::new(
        "artifact-routes-repo".to_string(),
        owner.id,
        "/git/artifact-routes-repo".to_string(),
    );
    let repo_id = repo.id;
    RepoQueries::create(&pool, &repo).await.unwrap();

    let pipeline = Pipeline {
        id: gitforge_common::PipelineId::new(),
        repo_id,
        name: "artifact-routes-pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: json!({"name": "artifact-routes-pipeline", "version": "1.0", "jobs": []}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&pool, &pipeline).await.unwrap();

    let run = PipelineRun::new(pipeline.id, repo_id, "webhook".to_string(), "0".repeat(40));
    PipelineRunQueries::create(&pool, &run).await.unwrap();

    let job = Job::new(run.id, "artifact-job".to_string());
    let job_id = job.id;
    JobQueries::create(&pool, &job).await.unwrap();

    // A second job owned by nobody's repo chain: its run points at a
    // pipeline of a different repository the intruder owns, proving the
    // admin bypass and cross-repo denial are both job-scoped.
    let other_repo = Repository::new(
        "artifact-other-repo".to_string(),
        intruder.id,
        "/git/artifact-other-repo".to_string(),
    );
    let other_repo_id = other_repo.id;
    RepoQueries::create(&pool, &other_repo).await.unwrap();
    let other_pipeline = Pipeline {
        id: gitforge_common::PipelineId::new(),
        repo_id: other_repo_id,
        name: "artifact-other-pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: json!({"name": "artifact-other-pipeline", "version": "1.0", "jobs": []}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&pool, &other_pipeline)
        .await
        .unwrap();
    let other_run = PipelineRun::new(
        other_pipeline.id,
        other_repo_id,
        "webhook".to_string(),
        "1".repeat(40),
    );
    PipelineRunQueries::create(&pool, &other_run).await.unwrap();
    let other_job = Job::new(other_run.id, "other-artifact-job".to_string());
    let other_job_id = other_job.id;
    JobQueries::create(&pool, &other_job).await.unwrap();

    let artifact_root = tempfile::tempdir().unwrap();
    let storage = Arc::new(FileStorage::new(artifact_root.path()).await.unwrap());
    let artifact_root = artifact_root;

    let auth = ApiAuth::new("test-secret");
    let token_for =
        |user: &User, role: &str| auth.generate_token(user.id, &user.username, role).unwrap();
    let app = ApiServer::new("test-secret", pool.clone())
        .with_storage_extension(storage.clone())
        .into_router();
    Fixture {
        app,
        storage,
        _artifact_root: artifact_root,
        owner_token: token_for(&owner, "developer"),
        admin_token: token_for(&admin, "admin"),
        intruder_token: token_for(&intruder, "developer"),
        job_id,
        other_job_id,
    }
}

fn artifact_for(job_id: JobId, name: &str, data: &[u8]) -> Artifact {
    Artifact {
        id: ArtifactId::new(),
        job_id,
        name: name.to_string(),
        path: format!("/api/artifacts/{name}"),
        checksum: format!("checksum-{}", data.len()),
        size_bytes: data.len() as u64,
        content_type: Some("application/octet-stream".to_string()),
        created_at: chrono::Utc::now(),
    }
}

async fn put_artifact(storage: &FileStorage, job_id: JobId, name: &str, data: &[u8]) -> ArtifactId {
    let artifact = artifact_for(job_id, name, data);
    let id = artifact.id;
    storage.put(&artifact, data).await.unwrap();
    id
}

async fn request(
    app: Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    let request = builder
        .body(Body::from(
            body.map(|v| v.to_string().into_bytes()).unwrap_or_default(),
        ))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

#[tokio::test]
async fn list_returns_owned_artifacts_and_filters_others() {
    let f = seed().await;
    put_artifact(&f.storage, f.job_id, "owned.bin", b"owned-data").await;
    put_artifact(&f.storage, f.other_job_id, "foreign.bin", b"foreign-data").await;

    let (status, body) = request(
        f.app.clone(),
        "GET",
        "/api/artifacts",
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert!(names.contains(&"owned.bin"));
    assert!(!names.contains(&"foreign.bin"));

    // The intruder sees only their own artifact.
    let (status, body) = request(
        f.app.clone(),
        "GET",
        "/api/artifacts",
        Some(&f.intruder_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert!(names.contains(&"foreign.bin"));
    assert!(!names.contains(&"owned.bin"));

    // Admins bypass job ownership entirely.
    let (status, body) = request(f.app, "GET", "/api/artifacts", Some(&f.admin_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert_eq!(names.len(), 2);
}

#[tokio::test]
async fn get_metadata_enforces_ownership_and_validates_ids() {
    let f = seed().await;
    let artifact_id = put_artifact(&f.storage, f.job_id, "meta.bin", b"meta").await;

    let (status, body) = request(
        f.app.clone(),
        "GET",
        &format!("/api/artifacts/{artifact_id}"),
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "meta.bin");
    assert_eq!(body["size_bytes"], 4);
    assert_eq!(body["job_id"], f.job_id.to_string());

    let (status, body) = request(
        f.app.clone(),
        "GET",
        &format!("/api/artifacts/{artifact_id}"),
        Some(&f.intruder_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");

    let (status, _) = request(
        f.app.clone(),
        "GET",
        &format!("/api/artifacts/{artifact_id}"),
        Some(&f.admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = request(
        f.app,
        "GET",
        "/api/artifacts/not-a-uuid",
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_id");
}

#[tokio::test]
async fn get_metadata_for_missing_artifact_is_not_found() {
    let f = seed().await;
    let (status, body) = request(
        f.app,
        "GET",
        &format!("/api/artifacts/{}", ArtifactId::new()),
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
}

/// Issue a request without parsing the body, for content and header assertions.
async fn request_raw(
    app: Router,
    method: &str,
    uri: &str,
    token: &str,
) -> axum::http::Response<Body> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    app.oneshot(request).await.unwrap()
}

#[tokio::test]
async fn download_streams_bytes_after_authorization() {
    let f = seed().await;
    let payload = b"artifact-bytes-0123456789";
    let artifact_id = put_artifact(&f.storage, f.job_id, "dl.bin", payload).await;
    let uri = format!("/api/artifacts/{artifact_id}/content");

    let response = request_raw(f.app.clone(), "GET", &uri, &f.owner_token).await;
    let status = response.status();
    let content_type = response.headers()["content-type"].clone();
    let head = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&head));
    assert_eq!(content_type, "application/octet-stream");
    assert_eq!(&head[..], payload);

    // Intruders get a not-found that does not leak existence.
    let response = request_raw(f.app.clone(), "GET", &uri, &f.intruder_token).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Invalid IDs are rejected before any lookup.
    let response = request_raw(f.app, "GET", "/api/artifacts/bad/content", &f.owner_token).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn download_missing_artifact_is_not_found() {
    let f = seed().await;
    let response = request_raw(
        f.app,
        "GET",
        &format!("/api/artifacts/{}/content", ArtifactId::new()),
        &f.owner_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_removes_artifact_for_owner_only() {
    let f = seed().await;
    let artifact_id = put_artifact(&f.storage, f.job_id, "doomed.bin", b"doomed").await;

    // Intruder cannot delete what they cannot see.
    let (status, body) = request(
        f.app.clone(),
        "DELETE",
        &format!("/api/artifacts/{artifact_id}"),
        Some(&f.intruder_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
    assert!(f.storage.get_metadata(artifact_id).await.is_ok());

    // Owner delete succeeds with 204 and the artifact is gone.
    let (status, body) = request(
        f.app.clone(),
        "DELETE",
        &format!("/api/artifacts/{artifact_id}"),
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);
    assert!(f.storage.get_metadata(artifact_id).await.is_err());

    // A missing artifact id deletes to not-found.
    let (status, _) = request(
        f.app.clone(),
        "DELETE",
        &format!("/api/artifacts/{}", ArtifactId::new()),
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Malformed ids are 400.
    let (status, _) = request(
        f.app,
        "DELETE",
        "/api/artifacts/nope",
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn job_artifacts_listing_respects_ownership() {
    let f = seed().await;
    put_artifact(&f.storage, f.job_id, "job-a.bin", b"a").await;
    put_artifact(&f.storage, f.job_id, "job-b.bin", b"bb").await;

    let (status, body) = request(
        f.app.clone(),
        "GET",
        &format!("/api/jobs/{}/artifacts", f.job_id),
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["job-a.bin", "job-b.bin"]);

    // Another user gets a non-committal not-found.
    let (status, body) = request(
        f.app.clone(),
        "GET",
        &format!("/api/jobs/{}/artifacts", f.job_id),
        Some(&f.intruder_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");

    // Admin sees the listing through the role bypass.
    let (status, _) = request(
        f.app.clone(),
        "GET",
        &format!("/api/jobs/{}/artifacts", f.job_id),
        Some(&f.admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Invalid job ids are 400.
    let (status, body) = request(
        f.app,
        "GET",
        "/api/jobs/zzz/artifacts",
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_id");
}

#[tokio::test]
async fn job_artifacts_for_missing_job_is_not_found() {
    let f = seed().await;
    let (status, _) = request(
        f.app,
        "GET",
        &format!("/api/jobs/{}/artifacts", JobId::new()),
        Some(&f.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
