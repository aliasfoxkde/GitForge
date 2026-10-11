//! Durable trigger settlement tests.
//!
//! The trigger endpoint records every accepted request durably before
//! publishing it, and the consumer settles each row according to why
//! processing failed: a committed `.gitforge.yml` that can never parse is
//! terminal (`failed`, visible immediately), while an environment failure
//! keeps its retry budget (`pending`, attempt counted). These tests drive
//! the real binary end to end — HTTP trigger, event consumer, committed
//! config loader — and assert the rows the service itself wrote. They also
//! pin the request-validation contract (4xx before anything is recorded)
//! and the fail-closed behavior when no trigger token is configured.

// Test-harness exemption, same discipline as the sibling suites:
// `allow-unwrap-in-tests` covers `#[test]` bodies, but boot/seed/fixture
// helper functions in a test target are neither `#[test]` fns nor
// `#[cfg(test)]`, a class the clippy.toml config cannot address. Setup
// failing IS the assertion -- a panic aborts the run loudly. Production
// code keeps the denies.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use gitforge_common::RepoId;

mod common;

/// A committed definition the YAML parser rejects. Every retry of a push
/// carrying this file fails identically, so the request must settle
/// terminally instead of consuming its budget.
const UNPARSEABLE_PIPELINE: &str = "name: [unclosed";

/// A committed definition that is not valid UTF-8. It cannot parse by
/// construction, so it is the same terminal class as unparseable YAML —
/// this leg pins that classification (it was once a plain git-spawn-shaped
/// error and burned the whole retry budget before surfacing).
const NON_UTF8_PIPELINE_BYTES: &[u8] = &[0xff, 0xfe, b'c', b'i', 0x00];

/// The harness trigger credential, shared with `ci_trigger_flow.rs`: the
/// value the spawned service is configured to require.
const HARNESS_TRIGGER_TOKEN: &str = "harness-trigger-token";

/// One seeded repository carrying a deliberately broken committed config.
struct BrokenRepo {
    repo_id: RepoId,
    commit_hash: String,
}

/// Environment for a spawned ci service.
struct CiService {
    child: tokio::process::Child,
    scheduler_port: u16,
    db_path: PathBuf,
    /// Repository committed with unparseable YAML.
    yaml_broken: BrokenRepo,
    /// Repository committed with non-UTF-8 config bytes.
    utf8_broken: BrokenRepo,
}

impl Drop for CiService {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn run_git(args: &[&str], cwd: &std::path::Path) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Commit `config_bytes` as `.gitforge.yml` on a fresh bare repository and
/// register it for `owner_id`.
async fn seed_broken_repo(
    base: &std::path::Path,
    git_root: &std::path::Path,
    name: &str,
    owner_id: gitforge_common::UserId,
    pool: &gitforge_db::Pool,
    config_bytes: &[u8],
) -> BrokenRepo {
    let bare = git_root.join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).expect("create bare dir");
    run_git(
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            bare.to_str().expect("utf8 bare path"),
        ],
        base,
    );
    let seed = base.join(format!("{name}-seed"));
    std::fs::create_dir_all(&seed).expect("create seed dir");
    run_git(&["init", "--initial-branch=main"], &seed);
    run_git(&["config", "user.email", "dev@example.com"], &seed);
    run_git(&["config", "user.name", "Settlement Harness"], &seed);
    std::fs::write(seed.join(".gitforge.yml"), config_bytes).expect("write broken config");
    run_git(&["add", "."], &seed);
    run_git(&["commit", "-m", "broken config seed"], &seed);
    run_git(
        &[
            "remote",
            "add",
            "origin",
            bare.to_str().expect("utf8 bare path"),
        ],
        &seed,
    );
    run_git(&["push", "origin", "main"], &seed);
    let commit_hash = run_git(&["rev-parse", "HEAD"], &seed);

    let repository = gitforge_db::models::Repository::new(
        name.to_string(),
        owner_id,
        bare.display().to_string(),
    );
    let repo_id = repository.id;
    gitforge_db::queries::RepoQueries::create(pool, &repository)
        .await
        .expect("create repository");
    BrokenRepo {
        repo_id,
        commit_hash,
    }
}

/// Spawn the real ci binary. `trigger_token = None` configures no trigger
/// credential at all (every candidate env var is removed), which must make
/// the endpoint fail closed.
async fn spawn_ci(trigger_token: Option<&str>) -> CiService {
    let unique = uuid::Uuid::new_v4();
    let base = std::env::temp_dir().join(format!("gitforge-ci-settle-{unique}"));
    let git_root = base.join("git");
    let workspaces = base.join("workspaces");
    let artifacts = base.join("artifacts");
    let db_path = base.join("gitforge.db");
    for dir in [&git_root, &workspaces, &artifacts] {
        std::fs::create_dir_all(dir).expect("create layout");
    }

    let pool = gitforge_db::Pool::new(&db_path.display().to_string())
        .await
        .expect("create sqlite pool");
    pool.migrate().await.expect("run migrations");
    let user = gitforge_db::models::User::new(
        "settlementowner".to_string(),
        "settlementowner@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .expect("create user");

    let yaml_broken = seed_broken_repo(
        &base,
        &git_root,
        "yaml-broken",
        user.id,
        &pool,
        UNPARSEABLE_PIPELINE.as_bytes(),
    )
    .await;
    let utf8_broken = seed_broken_repo(
        &base,
        &git_root,
        "utf8-broken",
        user.id,
        &pool,
        NON_UTF8_PIPELINE_BYTES,
    )
    .await;
    drop(pool);

    let scheduler_port = free_port();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ci"));
    command
        .env("SCHEDULER_PORT", scheduler_port.to_string())
        .env(
            "GITFORGE_DATABASE_URL",
            format!("sqlite:{}", db_path.display()),
        )
        .env("GITFORGE_ARTIFACT_ROOT", &artifacts)
        .env("GITFORGE_WORKSPACE_ROOT", &workspaces)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match trigger_token {
        Some(token) => {
            command.env("GITFORGE_TRIGGER_TOKEN", token);
        }
        None => {
            // The token resolver consults four variables in priority order;
            // a configured ambient value would silently re-enable auth.
            for name in [
                "GITFORGE_TRIGGER_TOKEN",
                "GITFORGE_CI_TRIGGER_TOKEN",
                "GITFORGE_SCHEDULER_OPERATOR_TOKEN",
                "GITFORGE_SCHEDULER_TOKEN",
            ] {
                command.env_remove(name);
            }
        }
    }
    let mut child = command.spawn().expect("spawn ci binary");

    let health = format!("http://127.0.0.1:{scheduler_port}/health");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if tokio::time::Instant::now() >= deadline {
            let _ = child.start_kill();
            panic!("ci service did not become healthy at {health}");
        }
        if let Ok(response) = reqwest::get(&health).await {
            if let Ok(body) = response.text().await {
                if body.contains("OK") {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    CiService {
        child,
        scheduler_port,
        db_path,
        yaml_broken,
        utf8_broken,
    }
}

async fn post_trigger(port: u16, token: Option<&str>, body: &str) -> (reqwest::StatusCode, String) {
    let url = format!("http://127.0.0.1:{port}/pipelines/trigger");
    let mut request = reqwest::Client::new().post(&url);
    if let Some(token) = token {
        request = request.header("x-gitforge-trigger-token", token);
    }
    let response = request
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .expect("send trigger request");
    let status = response.status();
    let text = response.text().await.expect("read response body");
    (status, text)
}

/// Wait until the durable request row reaches `want_status` and return it.
async fn settle(
    pool: &gitforge_db::Pool,
    trigger_id: uuid::Uuid,
    want_status: &str,
) -> gitforge_db::models::PipelineTriggerRequest {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let request = gitforge_db::queries::TriggerRequestQueries::get(pool, trigger_id)
            .await
            .expect("read trigger request");
        if let Some(request) = request {
            if request.status == want_status {
                return request;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "trigger request {trigger_id} never settled to '{want_status}'"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Both malformed committed-config shapes must settle terminally: the
/// request row is `failed` with the load error recorded, and no run row
/// exists — the failure is visible instead of burning the retry budget
/// invisibly and eventually dying as "budget exhausted".
#[tokio::test]
async fn test_unbuildable_committed_configs_settle_failed_without_a_run() {
    let mut service = spawn_ci(Some(HARNESS_TRIGGER_TOKEN)).await;

    // Trigger the YAML-broken repository. The request is accepted (it is
    // durable before the consumer ever sees it), but no run id can be
    // correlated: the consumer fails before creating one.
    let body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.yaml_broken.repo_id,
        "0".repeat(40),
        service.yaml_broken.commit_hash
    );
    let (status, response) =
        post_trigger(service.scheduler_port, Some(HARNESS_TRIGGER_TOKEN), &body).await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {response}");
    let payload: serde_json::Value =
        serde_json::from_str(&response).expect("parse trigger response");
    assert_eq!(payload["status"], "queued", "body: {response}");
    let yaml_trigger = payload["trigger_id"]
        .as_str()
        .expect("trigger_id in response")
        .to_string();

    // Trigger the non-UTF-8 repository the same way.
    let body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.utf8_broken.repo_id,
        "0".repeat(40),
        service.utf8_broken.commit_hash
    );
    let (status, response) =
        post_trigger(service.scheduler_port, Some(HARNESS_TRIGGER_TOKEN), &body).await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {response}");
    let payload: serde_json::Value =
        serde_json::from_str(&response).expect("parse trigger response");
    let utf8_trigger = payload["trigger_id"]
        .as_str()
        .expect("trigger_id in response")
        .to_string();

    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");

    // YAML: terminal, with the parse failure as the recorded cause.
    let settled = settle(
        &pool,
        yaml_trigger.parse().expect("uuid trigger id"),
        "failed",
    )
    .await;
    assert!(
        settled
            .error
            .as_deref()
            .is_some_and(|error| error.contains("invalid .gitforge.yml")),
        "the parse failure must be the recorded cause: {:?}",
        settled.error
    );
    assert!(settled.run_id.is_none(), "no run exists to attribute");

    // Non-UTF-8 bytes: the same terminal class, not a retryable one.
    let settled = settle(
        &pool,
        utf8_trigger.parse().expect("uuid trigger id"),
        "failed",
    )
    .await;
    assert!(
        settled
            .error
            .as_deref()
            .is_some_and(|error| error.contains("not valid UTF-8")),
        "the encoding failure must be the recorded cause: {:?}",
        settled.error
    );
    assert!(settled.run_id.is_none(), "no run exists to attribute");

    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert!(
        runs.is_empty(),
        "an unbuildable config must not create run rows: {runs:?}"
    );

    common::shutdown_gracefully(&mut service.child).await;
}

/// Malformed trigger requests are rejected before anything is recorded:
/// the 4xx contract guards a deletion sentinel, an unidentifiable
/// repository, and a caller-supplied checkout path outside the trusted
/// workspace roots.
#[tokio::test]
async fn test_trigger_http_contract_rejects_malformed_requests() {
    let mut service = spawn_ci(Some(HARNESS_TRIGGER_TOKEN)).await;
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");

    // An all-zero new hash is a ref-deletion sentinel, never a build.
    let body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.yaml_broken.repo_id,
        "0".repeat(40),
        "0".repeat(40)
    );
    let (status, response) =
        post_trigger(service.scheduler_port, Some(HARNESS_TRIGGER_TOKEN), &body).await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "body: {response}");
    assert!(response.contains("invalid_new_hash"), "body: {response}");

    // A repo_id that is not a UUID identifies nothing.
    let body = format!(
        "{{\"repo_id\":\"not-a-uuid\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        "0".repeat(40),
        service.yaml_broken.commit_hash
    );
    let (status, response) =
        post_trigger(service.scheduler_port, Some(HARNESS_TRIGGER_TOKEN), &body).await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "body: {response}");
    assert!(response.contains("invalid_repo_id"), "body: {response}");

    // A working directory outside the trusted workspace roots is an
    // untrusted checkout; the scheduler never builds into it.
    let body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":\"/etc\"}}",
        service.yaml_broken.repo_id,
        "0".repeat(40),
        service.yaml_broken.commit_hash
    );
    let (status, response) =
        post_trigger(service.scheduler_port, Some(HARNESS_TRIGGER_TOKEN), &body).await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "body: {response}");
    assert!(response.contains("invalid_workspace"), "body: {response}");

    let recorded = gitforge_db::queries::TriggerRequestQueries::find_active(
        &pool,
        service.yaml_broken.repo_id,
        &service.yaml_broken.commit_hash,
    )
    .await
    .expect("query active trigger requests");
    assert!(
        recorded.is_none(),
        "rejected requests must not be recorded durably: {recorded:?}"
    );

    common::shutdown_gracefully(&mut service.child).await;
}

/// With no trigger credential configured the endpoint fails closed: every
/// request is refused with 503 regardless of what the caller presents,
/// because an unauthenticated trigger boundary must never look usable.
#[tokio::test]
async fn test_trigger_without_configured_token_fails_closed() {
    let mut service = spawn_ci(None).await;

    let body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.yaml_broken.repo_id,
        "0".repeat(40),
        service.yaml_broken.commit_hash
    );
    for token in [
        None,
        Some(HARNESS_TRIGGER_TOKEN),
        Some("Bearer harness-trigger-token"),
    ] {
        let (status, response) = post_trigger(service.scheduler_port, token, &body).await;
        assert_eq!(
            status,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "token {token:?}: body: {response}"
        );
        assert!(
            response.contains("trigger_auth_not_configured"),
            "token {token:?}: body: {response}"
        );
    }

    common::shutdown_gracefully(&mut service.child).await;
}
