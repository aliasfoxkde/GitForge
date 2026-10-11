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

// Test-harness exemption, same discipline as the sibling suites:
// `allow-unwrap-in-tests` covers `#[test]` bodies, but boot/seed/fixture
// helper functions in a test target are neither `#[test]` fns nor
// `#[cfg(test)]`, a class the clippy.toml config cannot address. Setup
// failing IS the assertion -- a panic aborts the run loudly. Production
// code keeps the denies.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
async fn spawn_ci() -> CiService {
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
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ci"))
        .env("SCHEDULER_PORT", scheduler_port.to_string())
        .env(
            "GITFORGE_DATABASE_URL",
            format!("sqlite:{}", db_path.display()),
        )
        .env("GITFORGE_TRIGGER_TOKEN", "harness-trigger-token")
        .env("GITFORGE_ARTIFACT_ROOT", &artifacts)
        .env("GITFORGE_WORKSPACE_ROOT", &workspaces)
        .env("RUST_LOG", "warn")
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
        workspace_root: workspaces,
        repo_id,
        commit_hash,
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

    let jobs = gitforge_db::queries::JobQueries::list_by_run(&pool, runs[0].id)
        .await
        .expect("list jobs");
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

/// A 202 with `status: "queued"` and a null `pipeline_run_id` is a durable
/// pending request, and a client retrying the byte-identical trigger tuple
/// must land on the same in-flight request instead of minting a second run.
/// The gitforge-ci workflow correlates exactly this way: it re-POSTs the
/// same repo/ref/old_hash/new_hash until a response carries the run ID, so
/// the response schema and the dedup echo are load-bearing contract.
#[tokio::test]
async fn test_duplicate_trigger_dedups_onto_single_run() {
    let mut service = spawn_ci().await;

    let trigger_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.repo_id,
        "0".repeat(40),
        service.commit_hash
    );

    // Two identical triggers fired concurrently: the first creates the
    // durable request, the second must dedup onto it. The consumer cannot
    // settle the row between the two requests landing (its path spawns git
    // and writes several rows), so the racing duplicate observes an active
    // request.
    let port = service.scheduler_port;
    let (first, second) = tokio::join!(
        post_trigger(port, Some("harness-trigger-token"), &trigger_body),
        post_trigger(port, Some("harness-trigger-token"), &trigger_body)
    );

    let mut payloads = Vec::new();
    for (label, (status, body)) in [("first", first), ("second", second)] {
        assert_eq!(status, reqwest::StatusCode::ACCEPTED, "{label}: {body}");
        let payload: serde_json::Value =
            serde_json::from_str(&body).expect("parse trigger response");
        // The response schema every consumer of this endpoint parses.
        assert!(
            payload["status"] == "accepted" || payload["status"] == "queued",
            "{label}: status must be accepted|queued: {body}"
        );
        assert!(
            payload["pipeline_run_id"].is_null()
                || payload["pipeline_run_id"]
                    .as_str()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()),
            "{label}: pipeline_run_id must be null or a UUID: {body}"
        );
        let trigger_id = payload["trigger_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{label}: trigger_id must be present: {body}"));
        assert!(
            uuid::Uuid::parse_str(trigger_id).is_ok(),
            "{label}: trigger_id must be a UUID: {body}"
        );
        payloads.push(payload);
    }

    // The racing duplicate echoed the in-flight request, not a fresh one.
    let id_a = payloads[0]["trigger_id"].as_str().expect("trigger_id");
    let id_b = payloads[1]["trigger_id"].as_str().expect("trigger_id");
    assert_eq!(id_a, id_b, "both responses must name the same trigger");
    let deduped = payloads
        .iter()
        .find(|payload| payload["deduplicated"] == true)
        .unwrap_or_else(|| panic!("the racing duplicate must be marked deduplicated"));
    assert_eq!(
        deduped["pipeline_run_id"],
        serde_json::Value::Null,
        "an active request carries no run ID yet: {deduped}"
    );

    // Exactly one run exists for the commit: the duplicate produced no
    // second event and no second run.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let runs = loop {
        let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
            .await
            .expect("list runs");
        if !runs.is_empty() || tokio::time::Instant::now() >= deadline {
            break runs;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(runs.len(), 1, "dedup must yield exactly one run");
    assert_eq!(runs[0].repo_id, service.repo_id);
    assert_eq!(runs[0].commit_hash, service.commit_hash);

    // Whichever response correlated synchronously named that same run.
    let correlated = payloads
        .iter()
        .filter_map(|payload| payload["pipeline_run_id"].as_str())
        .next();
    if let Some(run_id) = correlated {
        assert_eq!(runs[0].id.to_string(), run_id);
    }

    common::shutdown_gracefully(&mut service.child).await;
}

/// A trigger whose repository git path is gone settles terminally inside
/// the consumer while the endpoint has already answered `queued` with only
/// a trigger id — the response shape cannot distinguish "pending" from
/// "failed". A retry of the identical tuple then dedups onto nothing (only
/// pending/processing rows count as active) and mints a FRESH trigger id.
/// The gitforge-ci correlation loop fails fast on exactly that signal: a
/// fresh echo means the original settled without a run the client could
/// identify, and no amount of retrying recovers it. Observed live
/// 2026-10-10: a repo row still pointed at a retired runtime-repos
/// directory, so every trigger failed `git path is unavailable`
/// (retryable=false) and no run ever existed.
#[tokio::test]
async fn test_settled_trigger_retry_mints_fresh_trigger() {
    let mut service = spawn_ci().await;
    let port = service.scheduler_port;

    // A second repository whose git path does not exist: event handling
    // fails non-retryably before any run or job row can be created.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let owner_id = gitforge_db::queries::RepoQueries::get(&pool, service.repo_id)
        .await
        .expect("read seeded repository")
        .expect("seeded repository exists")
        .owner_id;
    let ghost = gitforge_db::models::Repository::new(
        "ghost".to_string(),
        owner_id,
        "/nonexistent/ghost.git".to_string(),
    );
    let ghost_id = ghost.id;
    gitforge_db::queries::RepoQueries::create(&pool, &ghost)
        .await
        .expect("create ghost repository");
    drop(pool);

    let trigger_body = format!(
        "{{\"repo_id\":\"{ghost_id}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        "0".repeat(40),
        service.commit_hash
    );

    // First POST: accepted durably (202, queued) with only a trigger id.
    // The consumer settles the request terminally moments later, but the
    // response shape is already fixed at null pipeline_run_id.
    let (status, body) = post_trigger(port, Some("harness-trigger-token"), &trigger_body).await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let first: serde_json::Value = serde_json::from_str(&body).expect("parse first response");
    assert_eq!(first["status"], "queued", "body: {first}");
    assert!(
        first["pipeline_run_id"].is_null(),
        "no run can exist for a missing repository: {first}"
    );
    let original_id = first["trigger_id"]
        .as_str()
        .unwrap_or_else(|| panic!("trigger_id must be present: {first}"));
    let original_id = uuid::Uuid::parse_str(original_id).expect("trigger_id is a UUID");

    // The consumer marks the request failed, naming the git-path cause.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let settled = loop {
        let row = gitforge_db::queries::TriggerRequestQueries::get(&pool, original_id)
            .await
            .expect("read trigger request");
        match row {
            Some(row) if row.status == "failed" => break row,
            _ if tokio::time::Instant::now() >= deadline => {
                panic!("trigger request never settled to failed: {row:?}")
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    };
    assert!(
        settled
            .error
            .as_deref()
            .is_some_and(|error| error.contains("git path is unavailable")),
        "terminal cause must name the unavailable git path: {settled:?}"
    );

    // The identical retry dedups onto nothing and mints a fresh trigger:
    // exactly the signal the workflow treats as "original settled without
    // an identifiable run".
    let (status, body) = post_trigger(port, Some("harness-trigger-token"), &trigger_body).await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let second: serde_json::Value = serde_json::from_str(&body).expect("parse second response");
    assert_eq!(second["status"], "queued", "body: {second}");
    let retry_id = second["trigger_id"]
        .as_str()
        .unwrap_or_else(|| panic!("trigger_id must be present: {second}"));
    assert_ne!(
        uuid::Uuid::parse_str(retry_id).expect("retry trigger_id is a UUID"),
        original_id,
        "a settled trigger must not dedup the retry: {second}"
    );
    assert!(
        second["pipeline_run_id"].is_null(),
        "the retry mints a fresh request, not a correlated run: {second}"
    );

    // And no run was ever created for the ghost repository.
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert!(
        runs.iter().all(|run| run.repo_id != ghost_id),
        "no run may exist for a repository with no git path: {runs:?}"
    );

    common::shutdown_gracefully(&mut service.child).await;
}

/// The correlation read of a settled request: once the consumer has created
/// the run and marked the request `completed`, a retry addressed by
/// trigger_id must report that run instead of minting a second one. This is
/// the live 2026-10-10 failure shape (GitForge run 38106742159 passed while
/// the gitforge-ci poll job declared `trigger_lost`): the tuple dedup stops
/// matching at settlement, so an unaddressed re-POST minted a fresh request
/// — and a duplicate event — instead of surfacing the original run.
#[tokio::test]
async fn test_completed_trigger_state_read_returns_its_run() {
    let mut service = spawn_ci().await;
    let port = service.scheduler_port;

    let trigger_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.repo_id,
        "0".repeat(40),
        service.commit_hash
    );

    let (status, body) = post_trigger(port, Some("harness-trigger-token"), &trigger_body).await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let first: serde_json::Value = serde_json::from_str(&body).expect("parse first response");
    let trigger_id = first["trigger_id"]
        .as_str()
        .unwrap_or_else(|| panic!("trigger_id must be present: {first}"))
        .to_string();

    // Wait for the consumer's run row: the request is settled by now, which
    // is exactly the state the correlation read must survive.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let runs = loop {
        let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
            .await
            .expect("list runs");
        if !runs.is_empty() || tokio::time::Instant::now() >= deadline {
            break runs;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(runs.len(), 1, "the trigger must have created one run");
    let run_id = runs[0].id.to_string();

    let read_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null,\
          \"trigger_id\":\"{trigger_id}\"}}",
        service.repo_id,
        "0".repeat(40),
        service.commit_hash
    );
    let (status, body) = post_trigger(port, Some("harness-trigger-token"), &read_body).await;
    assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
    let state: serde_json::Value = serde_json::from_str(&body).expect("parse state response");
    assert_eq!(state["status"], "accepted", "body: {state}");
    assert_eq!(
        state["trigger_id"].as_str(),
        Some(trigger_id.as_str()),
        "the read must answer for the addressed request: {state}"
    );
    assert_eq!(
        state["pipeline_run_id"].as_str(),
        Some(run_id.as_str()),
        "the settled request must name its run: {state}"
    );

    // The read minted nothing: still exactly one run, and the settled
    // request stays invisible to the tuple dedup.
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert_eq!(runs.len(), 1, "the state read must not create a run");
    let active = gitforge_db::queries::TriggerRequestQueries::find_active(
        &pool,
        service.repo_id,
        &service.commit_hash,
    )
    .await
    .expect("find active trigger request");
    assert!(
        active.is_none(),
        "a settled request stays settled: {active:?}"
    );

    common::shutdown_gracefully(&mut service.child).await;
}

/// Terminal trigger failure transparency: once the ghost-repository request
/// has settled to `failed` with its cause, the correlation read addressed by
/// trigger_id must report exactly that — status `failed`, the recorded
/// cause, null run id — and mint nothing. Under the tuple-only contract this
/// read did not exist: the retry minted a fresh doomed request and the poll
/// job could only report `trigger_lost` with no cause at all.
#[tokio::test]
async fn test_failed_trigger_state_read_names_the_cause_without_minting() {
    let mut service = spawn_ci().await;
    let port = service.scheduler_port;

    // A second repository whose git path does not exist: event handling
    // fails non-retryably before any run or job row can be created.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let owner_id = gitforge_db::queries::RepoQueries::get(&pool, service.repo_id)
        .await
        .expect("read seeded repository")
        .expect("seeded repository exists")
        .owner_id;
    let ghost = gitforge_db::models::Repository::new(
        "ghost".to_string(),
        owner_id,
        "/nonexistent/ghost.git".to_string(),
    );
    let ghost_id = ghost.id;
    gitforge_db::queries::RepoQueries::create(&pool, &ghost)
        .await
        .expect("create ghost repository");
    drop(pool);

    let trigger_body = format!(
        "{{\"repo_id\":\"{ghost_id}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        "0".repeat(40),
        service.commit_hash
    );

    // First POST: accepted durably (202, queued) with only a trigger id.
    let (status, body) = post_trigger(port, Some("harness-trigger-token"), &trigger_body).await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let first: serde_json::Value = serde_json::from_str(&body).expect("parse first response");
    let original_id = first["trigger_id"]
        .as_str()
        .unwrap_or_else(|| panic!("trigger_id must be present: {first}"))
        .to_string();

    // The consumer marks the request failed, naming the git-path cause.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let settled = loop {
        let row = gitforge_db::queries::TriggerRequestQueries::get(
            &pool,
            uuid::Uuid::parse_str(&original_id).expect("trigger_id is a UUID"),
        )
        .await
        .expect("read trigger request");
        match row {
            Some(row) if row.status == "failed" => break row,
            _ if tokio::time::Instant::now() >= deadline => {
                panic!("trigger request never settled to failed: {row:?}")
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    };
    assert!(
        settled
            .error
            .as_deref()
            .is_some_and(|error| error.contains("git path is unavailable")),
        "terminal cause must name the unavailable git path: {settled:?}"
    );

    // The correlation read reports the settled failure and its cause.
    let read_body = format!(
        "{{\"repo_id\":\"{ghost_id}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null,\
          \"trigger_id\":\"{original_id}\"}}",
        "0".repeat(40),
        service.commit_hash
    );
    let (status, body) = post_trigger(port, Some("harness-trigger-token"), &read_body).await;
    assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
    let state: serde_json::Value = serde_json::from_str(&body).expect("parse state response");
    assert_eq!(state["status"], "failed", "body: {state}");
    assert_eq!(state["trigger_id"].as_str(), Some(original_id.as_str()));
    assert!(
        state["error"]
            .as_deref()
            .is_some_and(|error| error.contains("git path is unavailable")),
        "the read must surface the recorded cause: {state}"
    );
    assert!(
        state["pipeline_run_id"].is_null(),
        "no run exists for a missing git path: {state}"
    );

    // The read minted nothing: no fresh request, and no run for the ghost.
    let active = gitforge_db::queries::TriggerRequestQueries::find_active(
        &pool,
        ghost_id,
        &service.commit_hash,
    )
    .await
    .expect("find active trigger request");
    assert!(
        active.is_none(),
        "a failed request stays settled: {active:?}"
    );
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert!(
        runs.iter().all(|run| run.repo_id != ghost_id),
        "no run may exist for a repository with no git path: {runs:?}"
    );

    common::shutdown_gracefully(&mut service.child).await;
}

/// The correlation read of a request still in flight: `queued` with a null
/// run id — the same shape the dedup echo gives — and strictly inert: no
/// event is published and no run is minted for a request the read merely
/// inspected. The read is also bound to the (repo_id, new_hash) tuple it
/// claims (409 otherwise) and 404s on an unknown id, so a trigger id cannot
/// be replayed against another tuple.
#[tokio::test]
async fn test_pending_trigger_state_read_is_inert_and_tuple_bound() {
    let mut service = spawn_ci().await;
    let port = service.scheduler_port;

    // Seed a pending request directly: a durable row with no published
    // event, so nothing but a protocol violation could create a run here.
    let synthetic_hash = "f".repeat(40);
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let seeded = gitforge_db::models::PipelineTriggerRequest::new(
        service.repo_id,
        "refs/heads/main".to_string(),
        "0".repeat(40),
        synthetic_hash.clone(),
    );
    let seeded_id = seeded.id;
    let (_, was_new) =
        gitforge_db::queries::TriggerRequestQueries::create_or_existing(&pool, &seeded)
            .await
            .expect("seed trigger request");
    assert!(was_new, "the synthetic tuple must not dedup onto anything");

    let read_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{synthetic_hash}\",\
          \"working_dir\":null,\"trigger_id\":\"{seeded_id}\"}}",
        service.repo_id,
        "0".repeat(40),
    );
    let (status, body) = post_trigger(port, Some("harness-trigger-token"), &read_body).await;
    assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
    let state: serde_json::Value = serde_json::from_str(&body).expect("parse state response");
    assert_eq!(state["status"], "queued", "body: {state}");
    assert_eq!(
        state["trigger_id"].as_str(),
        Some(seeded_id.to_string().as_str())
    );
    assert!(
        state["pipeline_run_id"].is_null(),
        "an in-flight request carries no run id: {state}"
    );

    // The read published nothing: no run may appear for the seeded tuple.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let runs = loop {
        let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
            .await
            .expect("list runs");
        if !runs.is_empty() || tokio::time::Instant::now() >= deadline {
            break runs;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(
        runs.is_empty(),
        "a state read must not create or publish work: {runs:?}"
    );

    // The read is bound to its tuple: the same id against a different
    // new_hash answers 409 instead of surfacing another request's state.
    let mismatch_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null,\
          \"trigger_id\":\"{seeded_id}\"}}",
        service.repo_id,
        "0".repeat(40),
        "e".repeat(40),
    );
    let (status, body) = post_trigger(port, Some("harness-trigger-token"), &mismatch_body).await;
    assert_eq!(status, reqwest::StatusCode::CONFLICT, "body: {body}");
    let mismatch: serde_json::Value = serde_json::from_str(&body).expect("parse mismatch response");
    assert_eq!(
        mismatch["error"], "trigger_tuple_mismatch",
        "body: {mismatch}"
    );

    // An unknown id answers 404: there is nothing to correlate to.
    let unknown_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{synthetic_hash}\",\
          \"working_dir\":null,\"trigger_id\":\"{}\"}}",
        service.repo_id,
        "0".repeat(40),
        uuid::Uuid::new_v4(),
    );
    let (status, body) = post_trigger(port, Some("harness-trigger-token"), &unknown_body).await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "body: {body}");
    let unknown: serde_json::Value = serde_json::from_str(&body).expect("parse 404 response");
    assert_eq!(unknown["error"], "trigger_not_found", "body: {unknown}");

    common::shutdown_gracefully(&mut service.child).await;
}
