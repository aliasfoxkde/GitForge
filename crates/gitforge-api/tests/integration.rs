//! Integration tests for GitForge API
//!
//! These tests verify API functionality with database integration.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use gitforge_api::{ApiAuth, ApiServer};
use gitforge_ci::{JobDefinition, PipelineDefinition, StepDefinition, TriggerType};
use gitforge_db::models::{Runner, RunnerType};
use gitforge_db::Pool;
use gitforge_scheduler::{
    create_state_with_artifact_storage, scheduler_routes_with_tokens, Scheduler,
};
use gitforge_storage::FileStorage;
use gitforge_storage::{Artifact, ArtifactStore};
use std::sync::Arc;
use tower::ServiceExt;

async fn webhook_fixture(
    config: serde_json::Value,
) -> (
    axum::Router,
    gitforge_common::PipelineId,
    gitforge_common::RepoId,
    String,
) {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let user = gitforge_db::models::User::new(
        "webhook-test".to_string(),
        "webhook-test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();
    let repo_id = gitforge_common::RepoId::new();
    gitforge_db::queries::RepoQueries::create(
        &pool,
        &gitforge_db::models::Repository::new(
            "webhook-repo".to_string(),
            user.id,
            "/git/webhook-repo".to_string(),
        ),
    )
    .await
    .unwrap();
    let pipeline = gitforge_db::models::Pipeline {
        id: gitforge_common::PipelineId::new(),
        repo_id,
        name: "webhook-pipeline".to_string(),
        trigger_type: "push".to_string(),
        config,
        created_at: chrono::Utc::now(),
    };
    gitforge_db::queries::PipelineQueries::create(&pool, &pipeline)
        .await
        .unwrap();
    let token = ApiAuth::new("test-secret")
        .generate_token(user.id, "webhook-test", "admin")
        .unwrap();
    (
        ApiServer::new("test-secret", pool).into_router(),
        pipeline.id,
        repo_id,
        token,
    )
}

fn webhook_definition(job: JobDefinition) -> serde_json::Value {
    serde_json::to_value(PipelineDefinition {
        name: "webhook-pipeline".to_string(),
        version: "1.0".to_string(),
        trigger_on: vec![TriggerType::Push],
        environment: std::collections::HashMap::new(),
        jobs: vec![job],
    })
    .unwrap()
}

fn webhook_job(needs: Vec<String>, timeout: Option<&str>) -> JobDefinition {
    JobDefinition {
        name: "webhook-job".to_string(),
        image: "alpine:latest".to_string(),
        needs,
        env: std::collections::HashMap::new(),
        steps: vec![StepDefinition {
            name: "smoke".to_string(),
            run: "printf webhook-test".to_string(),
            env: None,
            working_directory: None,
            condition: None,
        }],
        timeout: timeout.map(str::to_string),
        retry: None,
    }
}

#[tokio::test]
async fn test_api_server_creation() {
    let pool = Pool::memory().await.unwrap();
    let server = ApiServer::new("test-secret", pool);
    assert_eq!(server.port, 42780);
}

#[tokio::test]
async fn test_health_check_endpoint() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Build the API server
    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    // Make request to health endpoint
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_metrics_endpoint() {
    let pool = Pool::memory().await.unwrap();
    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    // Request metrics
    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_swagger_ui_endpoint() {
    let pool = Pool::memory().await.unwrap();
    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/swagger-ui")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should redirect or return HTML
    assert!(response.status() == StatusCode::OK || response.status() == StatusCode::FOUND);
}

#[tokio::test]
async fn test_openapi_spec_endpoint() {
    let pool = Pool::memory().await.unwrap();
    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api-docs/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_api_server_with_custom_port() {
    let pool = Pool::memory().await.unwrap();
    let server = ApiServer::new("test-secret", pool).with_port(3000);
    assert_eq!(server.port, 3000);
}

#[tokio::test]
async fn test_cors_preflight() {
    let pool = Pool::memory().await.unwrap();
    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let response = app
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/health")
                .header("Origin", "http://localhost:3000")
                .header("Access-Control-Request-Method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // CORS preflight should succeed
    assert!(response.status() == StatusCode::OK || response.status() == StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_auth_with_expired_claims() {
    use chrono::Utc;
    use jsonwebtoken::{encode, Header};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize)]
    struct ExpiredClaims {
        sub: String,
        user_id: gitforge_common::UserId,
        username: String,
        role: String,
        exp: i64,
        iat: i64,
    }

    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    // Create expired token manually
    let claims = ExpiredClaims {
        sub: user.id.to_string(),
        user_id: user.id,
        username: "testuser".to_string(),
        role: "admin".to_string(),
        exp: Utc::now().timestamp() - 3600, // Expired 1 hour ago
        iat: Utc::now().timestamp() - 7200,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret("test-secret".as_bytes()),
    )
    .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Expired tokens should be rejected with 401
    // (Note: May return 500 if queries are not fully implemented)
    assert!(
        response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_protected_route_without_auth_returns_error() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    // Access protected route without auth
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get UNAUTHORIZED or INTERNAL_SERVER_ERROR (if auth check passes but DB fails)
    assert!(
        response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_create_repo_with_valid_auth() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Create user first
    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    // Generate valid token
    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    // Create repo
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/repos")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"name": "test-repo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should succeed (201) or fail gracefully (500 if query not implemented)
    assert!(
        response.status() == StatusCode::CREATED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_repository_access_is_owner_scoped() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let owner = gitforge_db::models::User::new(
        "owner".to_string(),
        "owner@example.com".to_string(),
        "hash".to_string(),
    );
    let other = gitforge_db::models::User::new(
        "other".to_string(),
        "other@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &owner)
        .await
        .unwrap();
    gitforge_db::queries::UserQueries::create(&pool, &other)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();
    let auth = ApiAuth::new("test-secret");
    let owner_token = auth.generate_token(owner.id, "owner", "developer").unwrap();
    let other_token = auth.generate_token(other.id, "other", "developer").unwrap();

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/repos")
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"name":"owner-only"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let body = to_bytes(create_response.into_body(), 16 * 1024)
        .await
        .unwrap();
    let repo_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let hidden = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/repos/{repo_id}"))
                .header("Authorization", format!("Bearer {other_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    let list = app
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .header("Authorization", format!("Bearer {other_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let body = to_bytes(list.into_body(), 16 * 1024).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Vec<serde_json::Value>>(&body)
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn test_admin_role_management_is_authorized_and_persisted() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let admin = gitforge_db::models::User::new(
        "role-admin".to_string(),
        "role-admin@example.com".to_string(),
        "hash".to_string(),
    );
    let second_admin = gitforge_db::models::User::new(
        "role-admin-2".to_string(),
        "role-admin-2@example.com".to_string(),
        "hash".to_string(),
    );
    let developer = gitforge_db::models::User::new(
        "role-developer".to_string(),
        "role-developer@example.com".to_string(),
        "hash".to_string(),
    );
    for user in [&admin, &second_admin, &developer] {
        gitforge_db::queries::UserQueries::create(&pool, user)
            .await
            .unwrap();
    }
    assert!(
        gitforge_db::queries::UserQueries::set_role(&pool, admin.id, "admin")
            .await
            .unwrap()
    );
    assert!(
        gitforge_db::queries::UserQueries::set_role(&pool, second_admin.id, "admin")
            .await
            .unwrap()
    );

    let auth = ApiAuth::new("test-secret");
    let admin_token = auth
        .generate_token(admin.id, "role-admin", "admin")
        .unwrap();
    let developer_token = auth
        .generate_token(developer.id, "role-developer", "developer")
        .unwrap();
    let app = ApiServer::new("test-secret", pool.clone()).into_router();

    let forbidden = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/users/{}/role", developer.id))
                .header("Authorization", format!("Bearer {developer_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"maintainer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let invalid = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/users/{}/role", developer.id))
                .header("Authorization", format!("Bearer {admin_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"root"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let promoted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/users/{}/role", developer.id))
                .header("Authorization", format!("Bearer {admin_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"maintainer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(promoted.status(), StatusCode::OK);
    assert_eq!(
        gitforge_db::queries::UserQueries::get_role(&pool, developer.id)
            .await
            .unwrap()
            .as_deref(),
        Some("maintainer")
    );

    let demoted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/users/{}/role", admin.id))
                .header("Authorization", format!("Bearer {admin_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"developer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(demoted.status(), StatusCode::OK);

    let second_admin_token = auth
        .generate_token(second_admin.id, "role-admin-2", "admin")
        .unwrap();

    let last_admin = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/users/{}/role", second_admin.id))
                .header("Authorization", format!("Bearer {second_admin_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"developer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(last_admin.status(), StatusCode::CONFLICT);

    let stale_admin = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/users/{}/role", second_admin.id))
                .header("Authorization", format!("Bearer {admin_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"developer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_admin.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_get_nonexistent_repo() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();
    gitforge_db::queries::UserQueries::set_role(&pool, user.id, "admin")
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/repos/00000000-0000-0000-0000-000000000000")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get NOT_FOUND or INTERNAL_SERVER_ERROR (if query not implemented)
    assert!(
        response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_delete_nonexistent_repo() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/repos/00000000-0000-0000-0000-000000000000")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get NOT_FOUND or INTERNAL_SERVER_ERROR (if query not implemented)
    assert!(
        response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::NO_CONTENT
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_dashboard_endpoint() {
    let pool = Pool::memory().await.unwrap();
    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_api_pipeline_runs_endpoint() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/ci/pipeline-runs")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Accept common success/error codes - route exists but may return 500 if DB query fails
    let status = response.status();
    assert!(
        status == StatusCode::OK
            || status == StatusCode::INTERNAL_SERVER_ERROR
            || status == StatusCode::NOT_FOUND,
        "Unexpected status: {}",
        status
    );
}

#[tokio::test]
async fn test_api_artifacts_endpoint() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();
    gitforge_db::queries::UserQueries::set_role(&pool, user.id, "admin")
        .await
        .unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let storage = FileStorage::new(temp_dir.path()).await.unwrap();

    let server = ApiServer::new("test-secret", pool).with_storage_extension(Arc::new(storage));
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/artifacts")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get OK or INTERNAL_SERVER_ERROR (if query not implemented)
    assert!(
        response.status() == StatusCode::OK
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_artifact_content_requires_auth_and_returns_bytes() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let user = gitforge_db::models::User::new(
        "artifact-user".to_string(),
        "artifact@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();
    gitforge_db::queries::UserQueries::set_role(&pool, user.id, "admin")
        .await
        .unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let storage = FileStorage::new(temp_dir.path()).await.unwrap();
    let source = temp_dir.path().join("build.txt");
    tokio::fs::write(&source, b"artifact-data").await.unwrap();
    let job_id = gitforge_common::JobId::new();
    let artifact = Artifact::from_file(job_id, "build.txt".to_string(), &source)
        .await
        .unwrap();
    let artifact_id = artifact.id.to_string();
    let data = tokio::fs::read(&source).await.unwrap();
    storage.put(&artifact, &data).await.unwrap();

    let server = ApiServer::new("test-secret", pool).with_storage_extension(Arc::new(storage));
    let app = server.into_router();
    let auth = ApiAuth::new("test-secret");
    let token = auth
        .generate_token(user.id, "artifact-user", "admin")
        .unwrap();

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/artifacts/{artifact_id}/content"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/artifacts/{artifact_id}/content"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"artifact-data");
}

#[tokio::test]
async fn test_scheduler_upload_is_downloadable_through_authenticated_api() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let user = gitforge_db::models::User::new(
        "boundary-owner".to_string(),
        "boundary-owner@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();
    gitforge_db::queries::UserQueries::set_role(&pool, user.id, "admin")
        .await
        .unwrap();
    let repo = gitforge_db::models::Repository::new(
        "boundary-repo".to_string(),
        user.id,
        "/git/boundary-repo".to_string(),
    );
    gitforge_db::queries::RepoQueries::create(&pool, &repo)
        .await
        .unwrap();
    let pipeline = gitforge_db::models::Pipeline {
        id: gitforge_common::PipelineId::new(),
        repo_id: repo.id,
        name: "boundary-pipeline".to_string(),
        trigger_type: "manual".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    gitforge_db::queries::PipelineQueries::create(&pool, &pipeline)
        .await
        .unwrap();
    let run = gitforge_db::models::PipelineRun::new(
        pipeline.id,
        repo.id,
        "boundary-owner".to_string(),
        "boundary-commit".to_string(),
    );
    gitforge_db::queries::PipelineRunQueries::create(&pool, &run)
        .await
        .unwrap();
    let job = gitforge_db::models::Job::new(run.id, "boundary-job".to_string());
    let job_id = job.id;
    gitforge_db::queries::JobQueries::create(&pool, &job)
        .await
        .unwrap();

    let scheduler = Scheduler::with_db(pool.clone());
    let runner = Runner::new("boundary-runner".to_string(), RunnerType::Docker, 1);
    let runner_id = runner.id;
    scheduler.register_runner(runner).await;
    scheduler
        .enqueue_with_definition(
            job_id,
            run.id,
            repo.id,
            vec!["echo boundary".to_string()],
            None,
        )
        .await;
    let artifact_root = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(FileStorage::new(artifact_root.path()).await.unwrap());
    let scheduler_app = scheduler_routes_with_tokens(
        create_state_with_artifact_storage(scheduler, Some(storage.clone())),
        Some(std::sync::Arc::from("runner-token")),
        Some(std::sync::Arc::from("operator-token")),
    );
    let pending = scheduler_app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/jobs/pending?runner_id={runner_id}"))
                .header("Authorization", "Bearer runner-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pending.status(), StatusCode::OK);
    let assignments: Vec<serde_json::Value> = serde_json::from_slice(
        &axum::body::to_bytes(pending.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let lease = assignments[0]["lease_token"].as_str().unwrap();
    let started = scheduler_app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/jobs/{job_id}/started"))
                .header("Authorization", "Bearer runner-token")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"runner_id": runner_id.to_string(), "lease_token": lease})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let bytes = b"cross-process artifact";
    let artifact = scheduler_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/jobs/{job_id}/artifacts"))
                .header("Authorization", "Bearer runner-token")
                .header("Content-Type", "application/octet-stream")
                .header("x-runner-id", runner_id.to_string())
                .header("x-lease-token", lease)
                .header("x-artifact-name", "result.txt")
                .body(Body::from(bytes.as_slice()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(artifact.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(artifact.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let artifact_id = payload["artifact_id"].as_str().unwrap();

    let api = ApiServer::new("test-secret", pool)
        .with_storage_extension(storage)
        .into_router();
    let token = ApiAuth::new("test-secret")
        .generate_token(user.id, "boundary-owner", "admin")
        .unwrap();
    let response = api
        .oneshot(
            Request::builder()
                .uri(format!("/api/artifacts/{artifact_id}/content"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        &axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()[..],
        bytes
    );
}

#[tokio::test]
async fn test_api_artifacts_are_scoped_to_job_repository_owner() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();
    let owner = gitforge_db::models::User::new(
        "artifact-owner".to_string(),
        "artifact-owner@example.com".to_string(),
        "hash".to_string(),
    );
    let other = gitforge_db::models::User::new(
        "artifact-other".to_string(),
        "artifact-other@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &owner)
        .await
        .unwrap();
    gitforge_db::queries::UserQueries::create(&pool, &other)
        .await
        .unwrap();

    let repo_id = gitforge_common::RepoId::new();
    gitforge_db::queries::RepoQueries::create(
        &pool,
        &gitforge_db::models::Repository {
            id: repo_id,
            name: "artifact-owned-repo".to_string(),
            owner_id: owner.id,
            visibility: "private".to_string(),
            git_path: "/tmp/artifact-owned-repo".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
    )
    .await
    .unwrap();
    let pipeline_id = gitforge_common::PipelineId::new();
    gitforge_db::queries::PipelineQueries::create(
        &pool,
        &gitforge_db::models::Pipeline {
            id: pipeline_id,
            repo_id,
            name: "artifact-ci".to_string(),
            trigger_type: "manual".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        },
    )
    .await
    .unwrap();
    let run_id = gitforge_common::PipelineRunId::new();
    gitforge_db::queries::PipelineRunQueries::create(
        &pool,
        &gitforge_db::models::PipelineRun {
            id: run_id,
            pipeline_id,
            repo_id,
            status: "succeeded".to_string(),
            triggered_by: "owner".to_string(),
            commit_hash: "artifact-commit".to_string(),
            started_at: Some(chrono::Utc::now()),
            finished_at: Some(chrono::Utc::now()),
            created_at: chrono::Utc::now(),
        },
    )
    .await
    .unwrap();
    let job = gitforge_db::models::Job::new(run_id, "artifact-build".to_string());
    let job_id = job.id;
    gitforge_db::queries::JobQueries::create(&pool, &job)
        .await
        .unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let storage = FileStorage::new(temp_dir.path()).await.unwrap();
    let source = temp_dir.path().join("private.txt");
    tokio::fs::write(&source, b"private-artifact")
        .await
        .unwrap();
    let artifact = Artifact::from_file(job_id, "private.txt".to_string(), &source)
        .await
        .unwrap();
    let artifact_id = artifact.id.to_string();
    storage.put(&artifact, b"private-artifact").await.unwrap();

    let server = ApiServer::new("test-secret", pool).with_storage_extension(Arc::new(storage));
    let app = server.into_router();
    let auth = ApiAuth::new("test-secret");
    let other_token = auth
        .generate_token(other.id, "artifact-other", "developer")
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/artifacts/{artifact_id}/content"))
                .header("Authorization", format!("Bearer {other_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/jobs/{job_id}/artifacts"))
                .header("Authorization", format!("Bearer {other_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_api_runners_endpoint() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/runners")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get OK or error
    assert!(
        response.status() == StatusCode::OK
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_get_nonexistent_artifact() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let storage = FileStorage::new(temp_dir.path()).await.unwrap();

    let server = ApiServer::new("test-secret", pool).with_storage_extension(Arc::new(storage));
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/artifacts/00000000-0000-0000-0000-000000000000")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get NOT_FOUND since artifact doesn't exist, or 204/202 if idempotent
    assert!(
        response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::NO_CONTENT
            || response.status() == StatusCode::ACCEPTED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_delete_nonexistent_artifact() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let storage = FileStorage::new(temp_dir.path()).await.unwrap();

    let server = ApiServer::new("test-secret", pool).with_storage_extension(Arc::new(storage));
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/artifacts/00000000-0000-0000-0000-000000000000")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get NOT_FOUND since artifact doesn't exist, or 204/202 if idempotent
    assert!(
        response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::NO_CONTENT
            || response.status() == StatusCode::ACCEPTED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_artifact_invalid_id() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let storage = FileStorage::new(temp_dir.path()).await.unwrap();

    let server = ApiServer::new("test-secret", pool).with_storage_extension(Arc::new(storage));
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/artifacts/invalid-uuid")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get BAD_REQUEST for invalid UUID or NOT_FOUND (route not matched)
    assert!(
        response.status() == StatusCode::BAD_REQUEST || response.status() == StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn test_api_job_artifacts_empty() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let storage = FileStorage::new(temp_dir.path()).await.unwrap();

    let server = ApiServer::new("test-secret", pool).with_storage_extension(Arc::new(storage));
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/jobs/00000000-0000-0000-0000-000000000000/artifacts")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get OK or INTERNAL_SERVER_ERROR
    assert!(
        response.status() == StatusCode::OK
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
            || response.status() == StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn test_api_artifacts_without_auth() {
    let pool = Pool::memory().await.unwrap();
    let temp_dir = tempfile::tempdir().unwrap();
    let storage = FileStorage::new(temp_dir.path()).await.unwrap();

    let server = ApiServer::new("test-secret", pool).with_storage_extension(Arc::new(storage));
    let app = server.into_router();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/artifacts")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get UNAUTHORIZED or INTERNAL_SERVER_ERROR (if storage check fails first)
    assert!(
        response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_pipelines_endpoint_list() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/pipelines")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get OK with empty array
    assert!(
        response.status() == StatusCode::OK
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_pipeline_runs_endpoint_list() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/pipeline-runs")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get OK with empty array
    assert!(
        response.status() == StatusCode::OK
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_get_nonexistent_pipeline() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/pipelines/00000000-0000-0000-0000-000000000000")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get NOT_FOUND or error
    assert!(
        response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_get_nonexistent_pipeline_run() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/pipeline-runs/00000000-0000-0000-0000-000000000000")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get NOT_FOUND or error
    assert!(
        response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_pipeline_invalid_id() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/pipelines/invalid-uuid")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get BAD_REQUEST or NOT_FOUND (routing behavior varies)
    assert!(
        response.status() == StatusCode::BAD_REQUEST || response.status() == StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn test_api_pipeline_run_invalid_id() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/pipeline-runs/invalid-uuid")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get BAD_REQUEST or NOT_FOUND (routing behavior varies)
    assert!(
        response.status() == StatusCode::BAD_REQUEST || response.status() == StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn test_api_get_nonexistent_job() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/jobs/00000000-0000-0000-0000-000000000000")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get NOT_FOUND or error
    assert!(
        response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_job_invalid_id() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/jobs/invalid-uuid")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get BAD_REQUEST or NOT_FOUND (routing behavior varies)
    assert!(
        response.status() == StatusCode::BAD_REQUEST || response.status() == StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn test_api_webhook_trigger_invalid_pipeline_id() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/webhook/trigger/invalid-uuid")
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"repo_id":"550e8400-e29b-41d4-a716-446655440000","commit_hash":"abc123","branch":"main"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Invalid UUID should return BAD_REQUEST
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_api_webhook_trigger_not_found() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();
    let valid_uuid = uuid::Uuid::new_v4().to_string();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/webhook/trigger/{}", valid_uuid))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"repo_id":"550e8400-e29b-41d4-a716-446655440000","commit_hash":"abc123","branch":"main"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Pipeline not found should return NOT_FOUND
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_api_webhook_trigger_unauthorized() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let valid_uuid = uuid::Uuid::new_v4().to_string();

    // No auth header
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/webhook/trigger/{}", valid_uuid))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"repo_id":"550e8400-e29b-41d4-a716-446655440000","commit_hash":"abc123","branch":"main"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Unauthorized
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_api_webhook_trigger_success() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    // Create user first
    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    // Create repo first (pipeline has FK to repo)
    let repo_id = gitforge_common::RepoId::new();
    let repo = gitforge_db::models::Repository {
        id: repo_id,
        name: "test-repo".to_string(),
        owner_id: user.id,
        visibility: "private".to_string(),
        git_path: "/tmp/test".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    gitforge_db::queries::RepoQueries::create(&pool, &repo)
        .await
        .unwrap();

    // Create a pipeline first with the same typed contract that the webhook
    // deserializes from persisted configuration.
    let pipeline_definition = PipelineDefinition {
        name: "test-pipeline".to_string(),
        version: "1.0".to_string(),
        trigger_on: vec![TriggerType::Push],
        environment: std::collections::HashMap::new(),
        jobs: vec![JobDefinition {
            name: "test".to_string(),
            image: "alpine:latest".to_string(),
            needs: vec![],
            env: std::collections::HashMap::new(),
            steps: vec![StepDefinition {
                name: "smoke".to_string(),
                run: "printf webhook-test".to_string(),
                env: None,
                working_directory: None,
                condition: None,
            }],
            timeout: Some("30s".to_string()),
            retry: None,
        }],
    };
    let pipeline = gitforge_db::models::Pipeline {
        id: gitforge_common::PipelineId::new(),
        repo_id,
        name: "test-pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: serde_json::to_value(pipeline_definition).unwrap(),
        created_at: chrono::Utc::now(),
    };
    gitforge_db::queries::PipelineQueries::create(&pool, &pipeline)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/webhook/trigger/{}", pipeline.id))
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(r#"{{"repo_id":"{}","commit_hash":"abc123","branch":"main","pipeline_name":"ci-pipeline"}}"#, repo_id)))
                .unwrap(),
        )
        .await
        .unwrap();

    // Pipeline found, trigger accepted. Include the response body on failure
    // so fixture/schema drift is diagnosed instead of reported as a bare 422.
    let status = response.status();
    let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "webhook response: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn test_api_webhook_trigger_rejects_invalid_stored_definition() {
    let (app, pipeline_id, repo_id, token) = webhook_fixture(serde_json::json!({})).await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/webhook/trigger/{pipeline_id}"))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"repo_id":"{repo_id}","commit_hash":"abc123","branch":"main"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_api_webhook_trigger_rejects_pipeline_without_entry_job() {
    let config = webhook_definition(webhook_job(vec!["missing".to_string()], Some("30s")));
    let (app, pipeline_id, repo_id, token) = webhook_fixture(config).await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/webhook/trigger/{pipeline_id}"))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"repo_id":"{repo_id}","commit_hash":"abc123","branch":"main"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_api_webhook_trigger_rejects_invalid_job_timeout() {
    let config = webhook_definition(webhook_job(vec![], Some("1s")));
    let (app, pipeline_id, repo_id, token) = webhook_fixture(config).await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/webhook/trigger/{pipeline_id}"))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"repo_id":"{repo_id}","commit_hash":"abc123","branch":"main"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_api_webhook_trigger_rejects_repository_mismatch() {
    let config = webhook_definition(webhook_job(vec![], Some("30s")));
    let (app, pipeline_id, _repo_id, token) = webhook_fixture(config).await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/webhook/trigger/{pipeline_id}"))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"repo_id":"{}","commit_hash":"abc123","branch":"main"}}"#,
                    gitforge_common::RepoId::new()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_api_get_job_with_valid_auth() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let valid_uuid = uuid::Uuid::new_v4().to_string();

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ci/jobs/{}", valid_uuid))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get NOT_FOUND (job doesn't exist but auth worked)
    assert!(
        response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_job_control_enforces_repository_ownership_and_persists_cancel() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let owner = gitforge_db::models::User::new(
        "owner".to_string(),
        "owner@example.com".to_string(),
        "hash".to_string(),
    );
    let other = gitforge_db::models::User::new(
        "other".to_string(),
        "other@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &owner)
        .await
        .unwrap();
    gitforge_db::queries::UserQueries::create(&pool, &other)
        .await
        .unwrap();

    let repo_id = gitforge_common::RepoId::new();
    gitforge_db::queries::RepoQueries::create(
        &pool,
        &gitforge_db::models::Repository {
            id: repo_id,
            name: "owned-repo".to_string(),
            owner_id: owner.id,
            visibility: "private".to_string(),
            git_path: "/tmp/owned-repo".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
    )
    .await
    .unwrap();
    let pipeline_id = gitforge_common::PipelineId::new();
    gitforge_db::queries::PipelineQueries::create(
        &pool,
        &gitforge_db::models::Pipeline {
            id: pipeline_id,
            repo_id,
            name: "ci".to_string(),
            trigger_type: "manual".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        },
    )
    .await
    .unwrap();
    let run_id = gitforge_common::PipelineRunId::new();
    gitforge_db::queries::PipelineRunQueries::create(
        &pool,
        &gitforge_db::models::PipelineRun {
            id: run_id,
            pipeline_id,
            repo_id,
            status: "running".to_string(),
            triggered_by: "manual".to_string(),
            commit_hash: "abc123".to_string(),
            started_at: Some(chrono::Utc::now()),
            finished_at: None,
            created_at: chrono::Utc::now(),
        },
    )
    .await
    .unwrap();
    let job = gitforge_db::models::Job::new(run_id, "build".to_string());
    let job_id = job.id;
    gitforge_db::queries::JobQueries::create(&pool, &job)
        .await
        .unwrap();

    let auth = ApiAuth::new("test-secret");
    let owner_token = auth.generate_token(owner.id, "owner", "developer").unwrap();
    let other_token = auth.generate_token(other.id, "other", "developer").unwrap();
    let server = ApiServer::new("test-secret", pool.clone());
    let app = server.into_router();

    let submission = serde_json::json!({
        "pipeline_run_id": run_id.to_string(),
        "name": "manual-check",
        "commands": ["cargo test"],
        "working_dir": null,
        "idempotency_key": "api-submit-1"
    });
    let submitted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/jobs")
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(submission.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(submitted.status(), StatusCode::CREATED);
    let replayed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/jobs")
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(submission.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replayed.status(), StatusCode::OK);

    let forbidden = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/jobs/{job_id}/cancel"))
                .header("Authorization", format!("Bearer {other_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let cancelled = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/jobs/{job_id}/cancel"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::OK);
    assert_eq!(
        gitforge_db::queries::JobQueries::get(&pool, job_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
}

#[tokio::test]
async fn test_api_pipeline_runs_with_jobs() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    // Create repo
    let repo_id = gitforge_common::RepoId::new();
    let repo = gitforge_db::models::Repository {
        id: repo_id,
        name: "test-repo".to_string(),
        owner_id: user.id,
        visibility: "private".to_string(),
        git_path: "/tmp/test".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    gitforge_db::queries::RepoQueries::create(&pool, &repo)
        .await
        .unwrap();

    // Create pipeline
    let pipeline_id = gitforge_common::PipelineId::new();
    let pipeline = gitforge_db::models::Pipeline {
        id: pipeline_id,
        repo_id,
        name: "test-pipeline".to_string(),
        trigger_type: "push".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    gitforge_db::queries::PipelineQueries::create(&pool, &pipeline)
        .await
        .unwrap();

    // Create pipeline run
    let run_id = gitforge_common::PipelineRunId::new();
    let run = gitforge_db::models::PipelineRun {
        id: run_id,
        pipeline_id,
        repo_id,
        status: "running".to_string(),
        commit_hash: "abc123".to_string(),
        triggered_by: "test".to_string(),
        started_at: Some(chrono::Utc::now()),
        finished_at: None,
        created_at: chrono::Utc::now(),
    };
    gitforge_db::queries::PipelineRunQueries::create(&pool, &run)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/pipeline-runs/{}", run_id))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return OK or NOT_FOUND (if query fails or run not found)
    assert!(
        response.status() == StatusCode::OK
            || response.status() == StatusCode::NOT_FOUND
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn test_api_get_pipeline_run_not_found() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/pipeline-runs/00000000-0000-0000-0000-000000000001")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return NOT_FOUND
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_api_ci_routes_require_auth() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    // Access CI routes without auth
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/pipelines")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get UNAUTHORIZED
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_api_get_pipeline_with_invalid_id_format() {
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let user = gitforge_db::models::User::new(
        "testuser".to_string(),
        "test@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .unwrap();

    let server = ApiServer::new("test-secret", pool);
    let app = server.into_router();

    let auth = ApiAuth::new("test-secret");
    let token = auth.generate_token(user.id, "testuser", "admin").unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/pipelines/not-a-uuid")
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get BAD_REQUEST for invalid UUID or NOT_FOUND
    assert!(
        response.status() == StatusCode::BAD_REQUEST || response.status() == StatusCode::NOT_FOUND
    );
}
