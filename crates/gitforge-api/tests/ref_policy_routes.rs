//! Integration tests for the ref-update policy routes (#240).
//!
//! These drive the real router with an in-memory database and real bare
//! repositories on disk: tightening `deny_non_fast_forward` must actually
//! write git's `receive.denyNonFastForwards` config, so the fixture
//! provisions repositories through the storage backend instead of inserting
//! dangling `git_path` rows.

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
use gitforge_common::{PipelineId, RepoId};
use gitforge_core::{FileStorageBackend, StorageBackend};
use gitforge_db::{
    models::{Pipeline, PipelineRun, Repository, User},
    queries::{PipelineQueries, PipelineRunQueries, RepoQueries, UserQueries},
    Pool,
};
use serde_json::{json, Value};
use std::sync::OnceLock;
use tower::ServiceExt;

/// One shared GIT_ROOT for the whole test binary: `update_ref_policy` reads
/// the environment variable on every call, and the tests in one binary share
/// a process. Repositories provisioned through the storage backend land in
/// here as real bare repositories so the git config write can succeed.
fn git_root() -> &'static str {
    static GIT_ROOT: OnceLock<String> = OnceLock::new();
    GIT_ROOT.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("gitforge-ref-policy-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Safety: single-threaded process-wide initialization via OnceLock.
        std::env::set_var("GIT_ROOT", &dir);
        dir.to_string_lossy().into_owned()
    })
}

struct Fixture {
    app: Router,
    pool: Pool,
    owner_token: String,
    intruder_token: String,
    repo_id: RepoId,
    owner_username: String,
    repo_name: String,
}

/// Seed an owner and an unrelated developer, plus a repository provisioned
/// as a real bare repository under the shared GIT_ROOT.
async fn seed(owner_username: &str, repo_name: &str) -> Fixture {
    git_root();
    let pool = Pool::memory().await.unwrap();
    pool.migrate().await.unwrap();

    let owner = User::new(
        owner_username.to_string(),
        format!("{owner_username}@example.com"),
        "hash".to_string(),
    );
    let intruder = User::new(
        format!("{owner_username}-intruder"),
        format!("{owner_username}-intruder@example.com"),
        "hash".to_string(),
    );
    for user in [&owner, &intruder] {
        UserQueries::create(&pool, user).await.unwrap();
    }

    let storage = FileStorageBackend::new(git_root());
    let repo_id = RepoId::new();
    storage.create(repo_id).await.unwrap();
    let git_path = storage.repo_path(repo_id);
    let repo = Repository::new_with_id(
        repo_id,
        repo_name.to_string(),
        owner.id,
        git_path.to_string_lossy().into_owned(),
    );
    RepoQueries::create(&pool, &repo).await.unwrap();

    let auth = ApiAuth::new("test-secret");
    let token_for =
        |user: &User, role: &str| auth.generate_token(user.id, &user.username, role).unwrap();
    let app = ApiServer::new("test-secret", pool.clone()).into_router();
    Fixture {
        app,
        pool,
        owner_token: token_for(&owner, "developer"),
        intruder_token: token_for(&intruder, "developer"),
        repo_id,
        owner_username: owner.username.clone(),
        repo_name: repo_name.to_string(),
    }
}

impl Fixture {
    fn policy_uri(&self) -> String {
        format!(
            "/api/repos/{}/{}/policy",
            self.owner_username, self.repo_name
        )
    }

    fn status_uri(&self, sha: &str) -> String {
        format!(
            "/api/repos/{}/{}/commits/{sha}/status",
            self.owner_username, self.repo_name
        )
    }

    /// Read the repository's git config for `receive.denyNonFastForwards`.
    async fn deny_config(&self) -> bool {
        let storage = FileStorageBackend::new(git_root());
        let repo = gitforge_core::StorageBackend::open(&storage, self.repo_id)
            .await
            .unwrap();
        repo.config()
            .unwrap()
            .get_bool("receive.denyNonFastForwards")
            .unwrap_or(false)
    }
}

async fn request_json(
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
    let response = app
        .oneshot(
            builder
                .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, parsed)
}

#[tokio::test]
async fn policy_roundtrips_and_writes_git_config() {
    let fixture = seed("policy-owner", "policy-repo").await;

    // Fresh repositories ship unpolicied.
    let (status, body) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.policy_uri(),
        Some(&fixture.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["required_checks"], json!([]));
    assert_eq!(body["deny_non_fast_forward"], json!(false));
    assert!(!fixture.deny_config().await);

    // Tightening writes the git config BEFORE persisting, so a policy that
    // claims to deny is always actually enforced.
    let (status, body) = request_json(
        fixture.app.clone(),
        "PATCH",
        &fixture.policy_uri(),
        Some(&fixture.owner_token),
        Some(json!({
            "required_checks": ["ci", "gates-and-release"],
            "deny_non_fast_forward": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["required_checks"], json!(["ci", "gates-and-release"]));
    assert_eq!(body["deny_non_fast_forward"], json!(true));
    assert!(fixture.deny_config().await);

    let (status, body) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.policy_uri(),
        Some(&fixture.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["required_checks"], json!(["ci", "gates-and-release"]));

    // Omitted fields keep their current values.
    let (status, _) = request_json(
        fixture.app.clone(),
        "PATCH",
        &fixture.policy_uri(),
        Some(&fixture.owner_token),
        Some(json!({"deny_non_fast_forward": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.policy_uri(),
        Some(&fixture.owner_token),
        None,
    )
    .await;
    assert_eq!(body["required_checks"], json!(["ci", "gates-and-release"]));
    assert_eq!(body["deny_non_fast_forward"], json!(false));
    assert!(!fixture.deny_config().await);

    // Clearing the check list round-trips too.
    let (status, body) = request_json(
        fixture.app.clone(),
        "PATCH",
        &fixture.policy_uri(),
        Some(&fixture.owner_token),
        Some(json!({"required_checks": []})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["required_checks"], json!([]));
    assert_eq!(body["deny_non_fast_forward"], json!(false));
}

#[tokio::test]
async fn policy_is_hidden_from_unauthorized_callers() {
    let fixture = seed("policy-guard", "guarded-repo").await;

    // A registered but unrelated developer gets 404, not 403, so repository
    // existence is not leaked.
    let (status, _) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.policy_uri(),
        Some(&fixture.intruder_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request_json(
        fixture.app.clone(),
        "PATCH",
        &fixture.policy_uri(),
        Some(&fixture.intruder_token),
        Some(json!({"required_checks": ["ci"]})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The policy was not changed by the rejected patch.
    let (status, body) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.policy_uri(),
        Some(&fixture.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["required_checks"], json!([]));

    // No token at all is rejected by the auth middleware.
    let (status, _) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.policy_uri(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn policy_patch_validates_required_check_names() {
    let fixture = seed("policy-validate", "validated-repo").await;

    for bad in ["", "   ", "ci;drop", "a\nb"] {
        let (status, body) = request_json(
            fixture.app.clone(),
            "PATCH",
            &fixture.policy_uri(),
            Some(&fixture.owner_token),
            Some(json!({"required_checks": [bad]})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "check name {bad:?}");
        assert_eq!(body["error"], json!("invalid_policy"));
    }

    // An oversized list is a configuration mistake, not a gate.
    let too_many: Vec<&str> = (0..=32).map(|_| "check").collect();
    let (status, _) = request_json(
        fixture.app.clone(),
        "PATCH",
        &fixture.policy_uri(),
        Some(&fixture.owner_token),
        Some(json!({"required_checks": too_many})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn commit_status_aggregates_required_checks() {
    let fixture = seed("status-owner", "status-repo").await;
    request_json(
        fixture.app.clone(),
        "PATCH",
        &fixture.policy_uri(),
        Some(&fixture.owner_token),
        Some(json!({"required_checks": ["ci"]})),
    )
    .await;

    let sha = "abcdef1234567890abcdef1234567890abcdef12";

    // No runs yet: the check exists but has never run.
    let (status, body) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.status_uri(sha),
        Some(&fixture.owner_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["satisfied"], json!(false));
    assert_eq!(body["required_checks"][0]["check"], json!("ci"));
    assert_eq!(body["required_checks"][0]["status"], json!(null));

    // A pending run is not green.
    let pipeline = Pipeline {
        id: PipelineId::new(),
        repo_id: fixture.repo_id,
        name: "ci".to_string(),
        trigger_type: "push".to_string(),
        config: json!({}),
        created_at: chrono::Utc::now(),
    };
    PipelineQueries::create(&fixture.pool, &pipeline)
        .await
        .unwrap();
    let run = PipelineRun::new(
        pipeline.id,
        fixture.repo_id,
        "push".to_string(),
        sha.to_string(),
    );
    PipelineRunQueries::create(&fixture.pool, &run)
        .await
        .unwrap();

    let (_, body) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.status_uri(sha),
        Some(&fixture.owner_token),
        None,
    )
    .await;
    assert_eq!(body["required_checks"][0]["status"], json!("pending"));
    assert_eq!(body["satisfied"], json!(false));

    // And a succeeded run satisfies the gate.
    PipelineRunQueries::update_status(&fixture.pool, run.id, "succeeded")
        .await
        .unwrap();
    let (_, body) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.status_uri(sha),
        Some(&fixture.owner_token),
        None,
    )
    .await;
    assert_eq!(body["required_checks"][0]["status"], json!("succeeded"));
    assert_eq!(body["satisfied"], json!(true));

    // A different commit still has nothing.
    let (_, body) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.status_uri(&"f".repeat(40)),
        Some(&fixture.owner_token),
        None,
    )
    .await;
    assert_eq!(body["satisfied"], json!(false));

    // Commit status is also hidden from unrelated developers.
    let (status, _) = request_json(
        fixture.app.clone(),
        "GET",
        &fixture.status_uri(sha),
        Some(&fixture.intruder_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
