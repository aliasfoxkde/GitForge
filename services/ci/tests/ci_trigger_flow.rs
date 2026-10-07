//! End-to-end CI trigger flow test.
//!
//! This test spawns the real `ci` binary against a temporary SQLite
//! database, bare git repository, workspace root, and artifact root, then
//! drives the same HTTP trigger endpoint the git-server calls after a
//! successful push. Nothing is mocked: the trigger auth middleware, the
//! in-process event consumer, the committed `.gitforge.yml` loader, the
//! workspace clone, and the durable scheduler enqueue all run in the real
//! service, and the run and job rows are asserted in the database the
//! service itself wrote.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use gitforge_common::{RepoId, UserId};

mod common;

/// Minimal but complete pipeline definition committed to the repository.
/// CI configuration is code: the run must be governed by this file, not by
/// a default pipeline the control plane would otherwise substitute.
const COMMITTED_PIPELINE: &str = r#"
name: harness-pipeline
version: "1.0"
trigger_on:
  - push
environment: {}
jobs:
  - name: harness-job
    image: alpine:latest
    steps:
      - name: smoke
        run: echo harness
"#;

/// Environment for a spawned ci service.
struct CiService {
    child: tokio::process::Child,
    scheduler_port: u16,
    db_path: PathBuf,
    git_root: PathBuf,
    workspace_root: PathBuf,
    repo_id: RepoId,
    commit_hash: String,
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

fn run_git(args: &[&str], cwd: &Path) -> String {
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

/// Prepare the database, bare repository with a committed `.gitforge.yml`,
/// and directory layout the service expects, then spawn the real ci binary.
/// `extra_env` entries override service configuration, e.g. the trigger
/// correlation window or additional credentials.
async fn spawn_ci_with(extra_env: &[(&str, &str)]) -> CiService {
    let unique = uuid::Uuid::new_v4();
    let base = std::env::temp_dir().join(format!("gitforge-ci-{unique}"));
    let git_root = base.join("git");
    let workspaces = base.join("workspaces");
    let artifacts = base.join("artifacts");
    let db_path = base.join("gitforge.db");
    for dir in [&git_root, &workspaces, &artifacts] {
        std::fs::create_dir_all(dir).expect("create layout");
    }

    // Seed the database the service will open: one owner and the repository
    // whose `git_path` points at the bare repository prepared below.
    let pool = gitforge_db::Pool::new(&db_path.display().to_string())
        .await
        .expect("create sqlite pool");
    pool.migrate().await.expect("run migrations");
    let user = gitforge_db::models::User::new(
        "triggerowner".to_string(),
        "triggerowner@example.com".to_string(),
        "hash".to_string(),
    );
    let user_id: UserId = user.id;
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .expect("create user");

    // Bare repository that receives the pushed branch, plus a seed commit
    // carrying the committed pipeline definition.
    let bare = git_root.join("harness.git");
    std::fs::create_dir_all(&bare).expect("create bare dir");
    run_git(
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            bare.to_str().unwrap(),
        ],
        &base,
    );
    let seed = base.join("seed");
    std::fs::create_dir_all(&seed).expect("create seed dir");
    run_git(&["init", "--initial-branch=main"], &seed);
    run_git(&["config", "user.email", "dev@example.com"], &seed);
    run_git(&["config", "user.name", "CI Trigger Harness"], &seed);
    std::fs::write(seed.join(".gitforge.yml"), COMMITTED_PIPELINE).expect("write pipeline");
    run_git(&["add", "."], &seed);
    run_git(&["commit", "-m", "seed commit with pipeline"], &seed);
    run_git(&["remote", "add", "origin", bare.to_str().unwrap()], &seed);
    run_git(&["push", "origin", "main"], &seed);
    let commit_hash = run_git(&["rev-parse", "HEAD"], &seed);

    let repository = gitforge_db::models::Repository::new(
        "harness".to_string(),
        user_id,
        bare.display().to_string(),
    );
    let repo_id = repository.id;
    gitforge_db::queries::RepoQueries::create(&pool, &repository)
        .await
        .expect("create repository");
    drop(pool);

    let scheduler_port = free_port();
    let mut service_command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ci"));
    service_command
        .env("SCHEDULER_PORT", scheduler_port.to_string())
        .env(
            "GITFORGE_DATABASE_URL",
            format!("sqlite:{}", db_path.display()),
        )
        .env("GITFORGE_TRIGGER_TOKEN", "harness-trigger-token")
        // The operator credential is separate from the trigger credential in
        // production; the harness pins both so tests can exercise the same
        // split the workflow uses.
        .env(
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN",
            "harness-operator-token",
        )
        .env("GITFORGE_ARTIFACT_ROOT", &artifacts)
        .env("GITFORGE_WORKSPACE_ROOT", &workspaces)
        .env("RUST_LOG", "warn");
    for (name, value) in extra_env {
        service_command.env(name, value);
    }
    let mut child = service_command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ci binary");

    // Wait for the scheduler HTTP API to come up.
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
        git_root,
        workspace_root: workspaces,
        repo_id,
        commit_hash,
    }
}

async fn spawn_ci() -> CiService {
    spawn_ci_with(&[]).await
}

async fn post_trigger(port: u16, token: Option<&str>, body: &str) -> (reqwest::StatusCode, String) {
    let url = format!("http://127.0.0.1:{port}/pipelines/trigger");
    let mut request = reqwest::Client::new().post(&url);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
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

async fn get_event_status(
    port: u16,
    token: Option<&str>,
    event_id: &str,
) -> (reqwest::StatusCode, String) {
    let url = format!("http://127.0.0.1:{port}/pipelines/events/{event_id}");
    let mut request = reqwest::Client::new().get(&url);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = request
        .header("accept", "application/json")
        .send()
        .await
        .expect("send event status request");
    let status = response.status();
    let text = response.text().await.expect("read response body");
    (status, text)
}

async fn get_run_status(
    port: u16,
    operator_token: &str,
    run_id: &str,
) -> (reqwest::StatusCode, String) {
    let url = format!("http://127.0.0.1:{port}/pipelines/runs/{run_id}");
    let response = reqwest::Client::new()
        .get(&url)
        .header("authorization", format!("Bearer {operator_token}"))
        .header("accept", "application/json")
        .send()
        .await
        .expect("send run status request");
    let status = response.status();
    let text = response.text().await.expect("read response body");
    (status, text)
}

/// Poll the correlation endpoint until the event leaves `pending`, mirroring
/// the workflow's bounded correlation loop.
async fn wait_correlation_terminal(service: &CiService, event_id: &str) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "event {event_id} never left pending within the correlation budget"
        );
        let (status, body) = get_event_status(
            service.scheduler_port,
            Some("harness-trigger-token"),
            event_id,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
        let payload: serde_json::Value =
            serde_json::from_str(&body).expect("parse correlation response");
        match payload["status"].as_str() {
            Some("pending") => tokio::time::sleep(Duration::from_millis(250)).await,
            Some("correlated") | Some("failed") => return payload,
            other => panic!("unexpected correlation status {other:?}: {body}"),
        }
    }
}

#[tokio::test]
async fn test_trigger_requires_token_and_runs_committed_pipeline() {
    let mut service = spawn_ci().await;

    let trigger_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.repo_id,
        "0".repeat(40),
        service.commit_hash
    );

    // Without the trigger token the endpoint must not start anything.
    let (status, body) = post_trigger(service.scheduler_port, None, &trigger_body).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "body: {body}");
    assert!(body.contains("trigger_auth_required"), "body: {body}");

    // With the token, the push event flows through the real in-process
    // consumer: the committed pipeline governs, the run is created, and its
    // id is reported synchronously.
    let (status, body) = post_trigger(
        service.scheduler_port,
        Some("harness-trigger-token"),
        &trigger_body,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("parse trigger response");
    assert_eq!(payload["status"], "accepted", "body: {body}");
    let run_id = payload["pipeline_run_id"]
        .as_str()
        .expect("pipeline_run_id in response")
        .to_string();

    // The service's durable rows: one run for the pushed commit with the
    // job from the committed definition enqueued for runners.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert_eq!(runs.len(), 1, "expected exactly one run");
    assert_eq!(runs[0].id.to_string(), run_id);
    assert_eq!(runs[0].repo_id, service.repo_id);
    assert_eq!(runs[0].commit_hash, service.commit_hash);

    // Correlation means the durable run row exists; workspace preparation and
    // job planning continue asynchronously after that point. Wait for the
    // committed definition to be enqueued rather than racing the consumer.
    let jobs = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let jobs = gitforge_db::queries::JobQueries::list_by_run(&pool, runs[0].id)
                .await
                .expect("list jobs");
            if !jobs.is_empty() {
                break jobs;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("pipeline job was not enqueued within 60s");
    assert_eq!(jobs.len(), 1, "committed pipeline has one job");
    assert_eq!(jobs[0].image, "alpine:latest");
    assert!(
        jobs[0]
            .commands
            .iter()
            .any(|command| command.contains("echo harness")),
        "job commands must come from the committed definition: {:?}",
        jobs[0].commands
    );

    // The run's workspace was really cloned from the bare repository and
    // checked out at the pushed commit.
    let workspace = service.workspace_root.join(runs[0].id.to_string());
    assert!(
        workspace.join(".git").exists(),
        "run workspace must be a git clone: {}",
        workspace.display()
    );
    let checked_out =
        std::fs::read_to_string(workspace.join(".gitforge.yml")).expect("workspace pipeline file");
    assert_eq!(checked_out, COMMITTED_PIPELINE);

    common::shutdown_gracefully(&mut service.child).await;
}

/// Regression for the accepted-but-not-yet-correlated trigger failure: a 202
/// with `pipeline_run_id: null` used to be unresolvable — the correlation
/// record was dropped when the synchronous window elapsed, so consumers
/// treated a legitimate accepted event as an enqueue failure. The event id
/// must stay resolvable to the durable run the consumer creates afterwards,
/// under the same trigger credential, without re-triggering.
#[tokio::test]
async fn test_queued_trigger_correlates_later_and_run_becomes_pollable() {
    // A zero-length correlation window forces the queued path while the
    // consumer still creates the run through the normal flow.
    let mut service = spawn_ci_with(&[("GITFORGE_TRIGGER_CORRELATION_WINDOW_SECS", "0")]).await;

    let trigger_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.repo_id,
        "0".repeat(40),
        service.commit_hash
    );

    let (status, body) = post_trigger(
        service.scheduler_port,
        Some("harness-trigger-token"),
        &trigger_body,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("parse trigger response");
    assert_eq!(payload["status"], "queued", "body: {body}");
    assert!(
        payload["pipeline_run_id"].is_null(),
        "a queued response carries no run id: {body}"
    );
    let event_id = payload["event_id"]
        .as_str()
        .expect("event_id in queued response")
        .to_string();

    // The correlation lookup is part of the trigger contract and shares its
    // credential: unauthenticated, malformed, and unknown lookups fail.
    let (status, _) = get_event_status(service.scheduler_port, None, &event_id).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    let (status, body) = get_event_status(
        service.scheduler_port,
        Some("harness-trigger-token"),
        "not-a-uuid",
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "body: {body}");
    let (status, body) = get_event_status(
        service.scheduler_port,
        Some("harness-trigger-token"),
        &uuid::Uuid::new_v4().to_string(),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "body: {body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).expect("parse 404 body")["error"],
        "event_not_found"
    );

    // The accepted event still creates its durable run; the correlation
    // endpoint must expose it without any second trigger.
    let correlation = wait_correlation_terminal(&service, &event_id).await;
    assert_eq!(correlation["status"], "correlated", "{correlation}");
    let run_id = correlation["pipeline_run_id"]
        .as_str()
        .expect("correlated response carries the run id")
        .to_string();

    // Exactly one run exists: a delayed correlation never re-triggered the
    // event into a duplicate pipeline run.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert_eq!(runs.len(), 1, "delayed correlation must not duplicate runs");
    assert_eq!(runs[0].id.to_string(), run_id);
    assert_eq!(runs[0].commit_hash, service.commit_hash);
    drop(pool);

    // The correlated run is pollable at the operator endpoint the workflow
    // uses, so GitHub CI can gate on its terminal state.
    let (status, body) =
        get_run_status(service.scheduler_port, "harness-operator-token", &run_id).await;
    assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
    let run: serde_json::Value = serde_json::from_str(&body).expect("parse run response");
    assert_eq!(run["commit_hash"], service.commit_hash);
    assert!(
        ["pending", "running"].contains(&run["status"].as_str().unwrap_or_default()),
        "a freshly correlated run is not terminal: {body}"
    );

    common::shutdown_gracefully(&mut service.child).await;
}

/// When the consumer cannot create a durable run (here: an unparseable
/// committed pipeline definition), the event must report `failed` instead of
/// staying pending, so the workflow fails closed rather than burning its
/// whole correlation budget or, worse, reporting success.
#[tokio::test]
async fn test_queued_trigger_reports_failed_when_run_creation_fails() {
    let mut service = spawn_ci_with(&[("GITFORGE_TRIGGER_CORRELATION_WINDOW_SECS", "0")]).await;

    // Commit an invalid pipeline definition on a fresh branch of the bare
    // repository. CI configuration is code: an unparseable definition must
    // fail run creation.
    let seed = service.git_root.join("bad-pipeline-seed");
    std::fs::create_dir_all(&seed).expect("create seed dir");
    run_git(&["init", "--initial-branch=main"], &seed);
    run_git(&["config", "user.email", "dev@example.com"], &seed);
    run_git(&["config", "user.name", "CI Trigger Harness"], &seed);
    std::fs::write(
        seed.join(".gitforge.yml"),
        "jobs: [this is: not: valid: yaml",
    )
    .expect("write invalid pipeline");
    run_git(&["add", "."], &seed);
    run_git(&["commit", "-m", "commit with unparseable pipeline"], &seed);
    let bare = service.git_root.join("harness.git");
    run_git(&["remote", "add", "origin", bare.to_str().unwrap()], &seed);
    run_git(&["push", "origin", "main:refs/heads/bad-pipeline"], &seed);
    let bad_hash = run_git(&["rev-parse", "HEAD"], &seed);

    let trigger_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/bad-pipeline\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.repo_id,
        "0".repeat(40),
        bad_hash
    );
    let (status, body) = post_trigger(
        service.scheduler_port,
        Some("harness-trigger-token"),
        &trigger_body,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("parse trigger response");
    assert_eq!(payload["status"], "queued", "body: {body}");
    let event_id = payload["event_id"]
        .as_str()
        .expect("event_id in queued response")
        .to_string();

    let correlation = wait_correlation_terminal(&service, &event_id).await;
    assert_eq!(correlation["status"], "failed", "{correlation}");
    let message = correlation["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("run creation failed"),
        "failure reason must be surfaced: {message}"
    );

    // No durable run was created for the failed event, and no amount of
    // correlation polling can manufacture one.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert_eq!(runs.len(), 0, "a failed event must not leave a run row");

    common::shutdown_gracefully(&mut service.child).await;
}
