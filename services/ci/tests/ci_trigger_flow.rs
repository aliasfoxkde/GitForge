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
use std::time::{Duration, Instant};

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

/// Mirror of the trigger-request store the ci binary creates at startup
/// (`TRIGGER_REQUEST_STORE_DDL` in `services/ci/src/main.rs`), so a fixture
/// can write rows before the first boot observes them. The schema is the
/// status endpoint's contract; a change here must land in both.
const TRIGGER_REQUEST_STORE_DDL: &str = r#"
    CREATE TABLE IF NOT EXISTS ci_trigger_requests (
        id TEXT PRIMARY KEY,
        event_id TEXT NOT NULL,
        repo_id TEXT NOT NULL,
        ref_name TEXT NOT NULL,
        new_hash TEXT NOT NULL,
        status TEXT NOT NULL,
        pipeline_run_id TEXT,
        error TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )
"#;

/// A trigger-request row written into the database before the service boots,
/// so the binary's startup sweeps run against known state.
struct SeedTriggerRequest {
    event_id: uuid::Uuid,
    status: &'static str,
    created_at: chrono::DateTime<chrono::Utc>,
    error: Option<&'static str>,
}

/// Prepare the database, bare repository with a committed `.gitforge.yml`,
/// and directory layout the service expects, then spawn the real ci binary.
async fn spawn_ci() -> CiService {
    spawn_ci_with(&[], true).await
}

/// Spawn the service with the extra environment a test needs.
///
/// The trigger credential, workspace root, artifact root, and layout are
/// always configured. `with_database` controls `GITFORGE_DATABASE_URL`: the
/// durable deployment under test correlates triggers, while the off case is
/// how the status endpoint's no-store behavior is exercised.
async fn spawn_ci_with(extra_env: &[(&str, &str)], with_database: bool) -> CiService {
    spawn_ci_impl(extra_env, with_database, &[]).await
}

/// Spawn the service against a database pre-seeded with trigger-request rows,
/// so the startup sweeps observe state written before this process existed.
async fn spawn_ci_seeded(extra_env: &[(&str, &str)], seeds: &[SeedTriggerRequest]) -> CiService {
    spawn_ci_impl(extra_env, true, seeds).await
}

/// Layout and spawn used by every variant: the database (optionally
/// pre-seeded), the bare repository, and the real ci binary.
async fn spawn_ci_impl(
    extra_env: &[(&str, &str)],
    with_database: bool,
    seeds: &[SeedTriggerRequest],
) -> CiService {
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

    // Pre-boot fixture rows land before the binary starts, so its startup
    // sweeps run against rows no live process could have written.
    if !seeds.is_empty() {
        assert!(with_database, "seeding trigger requests requires the store");
        sqlx::query(TRIGGER_REQUEST_STORE_DDL)
            .execute(pool.pool())
            .await
            .expect("create trigger-request fixture table");
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_ci_trigger_requests_event_id \
             ON ci_trigger_requests (event_id)",
        )
        .execute(pool.pool())
        .await
        .expect("create trigger-request fixture index");
        for seed in seeds {
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query(
                "INSERT INTO ci_trigger_requests \
                 (id, event_id, repo_id, ref_name, new_hash, status, pipeline_run_id, error, \
                  created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, ?)",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(seed.event_id.to_string())
            .bind(repo_id.to_string())
            .bind("refs/heads/main")
            .bind(&commit_hash)
            .bind(seed.status)
            .bind(seed.error)
            .bind(seed.created_at.to_rfc3339())
            .bind(&now)
            .execute(pool.pool())
            .await
            .expect("seed trigger request row");
        }
    }
    drop(pool);

    let scheduler_port = free_port();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ci"));
    command
        .env("SCHEDULER_PORT", scheduler_port.to_string())
        .env("GITFORGE_TRIGGER_TOKEN", "harness-trigger-token")
        .env("GITFORGE_ARTIFACT_ROOT", &artifacts)
        .env("GITFORGE_WORKSPACE_ROOT", &workspaces)
        .env("RUST_LOG", "warn");
    if with_database {
        command.env(
            "GITFORGE_DATABASE_URL",
            format!("sqlite:{}", db_path.display()),
        );
    }
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let mut child = command
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
    post_trigger_with_header(
        port,
        token.map(|token| ("x-gitforge-trigger-token", token)),
        body,
    )
    .await
}

/// POST the trigger with one credential header. `Authorization` is how the
/// workflow submits and reads; the dedicated trigger header is the git-server
/// compatibility form.
async fn post_trigger_with_header(
    port: u16,
    header: Option<(&str, &str)>,
    body: &str,
) -> (reqwest::StatusCode, String) {
    let url = format!("http://127.0.0.1:{port}/pipelines/trigger");
    let mut request = reqwest::Client::new().post(&url);
    if let Some((name, value)) = header {
        request = request.header(name, value);
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

/// GET the durable lifecycle of one trigger request under `Authorization`.
async fn get_trigger_request(
    port: u16,
    trigger_id: &str,
    credential: Option<&str>,
) -> (reqwest::StatusCode, String) {
    let url = format!("http://127.0.0.1:{port}/pipelines/trigger-requests/{trigger_id}");
    let mut request = reqwest::Client::new().get(&url);
    if let Some(credential) = credential {
        request = request.header("Authorization", credential);
    }
    let response = request.send().await.expect("send status request");
    let status = response.status();
    let text = response.text().await.expect("read response body");
    (status, text)
}

/// The trigger id a seeded row was written under: the fixture mints the row
/// id itself, so tests address rows through their known event ids.
async fn seeded_trigger_id(pool: &gitforge_db::Pool, event_id: uuid::Uuid) -> String {
    use sqlx::Row;
    sqlx::query("SELECT id FROM ci_trigger_requests WHERE event_id = ?")
        .bind(event_id.to_string())
        .fetch_one(pool.pool())
        .await
        .expect("seeded trigger request row")
        .try_get("id")
        .expect("row id")
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

/// The trigger-correlation contract: every accepted trigger carries a stable
/// `trigger_id`, a repeat trigger while the request is open deduplicates into
/// it, and the durable lifecycle is readable under the operator credential
/// only — the trigger credential submits work and can never read, and the
/// operator credential can never write.
#[tokio::test]
async fn test_trigger_request_lifecycle_and_credential_separation() {
    let mut service = spawn_ci_with(
        &[(
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN",
            "harness-operator-token",
        )],
        true,
    )
    .await;

    let trigger_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.repo_id,
        "0".repeat(40),
        service.commit_hash
    );
    let operator = "Bearer harness-operator-token";

    // The operator credential must not submit work.
    let (status, body) = post_trigger_with_header(
        service.scheduler_port,
        Some(("Authorization", operator)),
        &trigger_body,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "body: {body}");
    assert!(body.contains("trigger_auth_required"), "body: {body}");

    // The trigger credential submits through the production form,
    // `Authorization: Bearer`, and the response carries a stable id that is
    // present whether or not the run was correlated in time.
    let (status, body) = post_trigger_with_header(
        service.scheduler_port,
        Some(("Authorization", "Bearer harness-trigger-token")),
        &trigger_body,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("parse trigger response");
    let trigger_id = payload["trigger_id"]
        .as_str()
        .expect("trigger_id in response")
        .to_string();
    assert!(
        uuid::Uuid::parse_str(&trigger_id).is_ok(),
        "trigger_id must be a UUID: {body}"
    );
    assert_eq!(payload["deduplicated"], false, "body: {body}");

    // A repeat trigger while the request is open collapses into the same id
    // instead of planning a second run for the same commit. It arrives on the
    // git-server compatibility header, which must keep submitting too.
    let (status, body) = post_trigger(
        service.scheduler_port,
        Some("harness-trigger-token"),
        &trigger_body,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let repeat: serde_json::Value = serde_json::from_str(&body).expect("parse repeat response");
    assert_eq!(repeat["status"], "deduplicated", "body: {body}");
    assert_eq!(repeat["trigger_id"], payload["trigger_id"], "body: {body}");
    assert!(
        repeat.get("pipeline_run_id").is_none(),
        "a deduplicated answer plans no run: {body}"
    );

    // Reads are an operator capability: no credential, or the trigger
    // credential on either header, must be refused.
    let (status, body) = get_trigger_request(service.scheduler_port, &trigger_id, None).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "body: {body}");
    assert!(
        body.contains("trigger_request_auth_required"),
        "body: {body}"
    );
    let (status, body) = get_trigger_request(
        service.scheduler_port,
        &trigger_id,
        Some("Bearer harness-trigger-token"),
    )
    .await;
    assert_eq!(
        status,
        reqwest::StatusCode::UNAUTHORIZED,
        "the trigger credential must not read: {body}"
    );
    let (status, body) = get_trigger_request(
        service.scheduler_port,
        &trigger_id,
        Some("harness-trigger-token"),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "body: {body}");

    // Malformed and unknown ids are distinct, and neither is an auth problem.
    let (status, body) =
        get_trigger_request(service.scheduler_port, "not-a-uuid", Some(operator)).await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "body: {body}");
    assert!(body.contains("invalid_trigger_id"), "body: {body}");
    let unknown = uuid::Uuid::new_v4().to_string();
    let (status, body) =
        get_trigger_request(service.scheduler_port, &unknown, Some(operator)).await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "body: {body}");
    assert!(body.contains("trigger_request_not_found"), "body: {body}");

    // The operator credential reads the durable lifecycle, and the run the
    // consumer planned is linked into it.
    let mut lifecycle = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() >= deadline {
            panic!("trigger request never reached processing: {lifecycle}");
        }
        let (status, body) =
            get_trigger_request(service.scheduler_port, &trigger_id, Some(operator)).await;
        assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("parse lifecycle");
        if parsed["status"] == "processing" && parsed["pipeline_run_id"].is_string() {
            lifecycle = body;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let lifecycle_payload: serde_json::Value =
        serde_json::from_str(&lifecycle).expect("parse lifecycle");
    assert_eq!(
        lifecycle_payload["repo_id"].as_str(),
        Some(service.repo_id.to_string().as_str()),
        "body: {lifecycle}"
    );
    assert_eq!(
        lifecycle_payload["new_hash"].as_str(),
        Some(service.commit_hash.as_str()),
        "body: {lifecycle}"
    );
    assert!(
        !lifecycle.contains("harness-trigger-token")
            && !lifecycle.contains("harness-operator-token"),
        "no response may echo a credential: {lifecycle}"
    );

    // The correlated id is a real durable run row this service wrote, and the
    // repeated POST above must not have planned a second one for the same
    // commit.
    let run_id = lifecycle_payload["pipeline_run_id"]
        .as_str()
        .expect("pipeline_run_id in lifecycle")
        .to_string();
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert_eq!(
        runs.len(),
        1,
        "a repeated POST must not create a second pipeline run: {:?}",
        runs.iter()
            .map(|run| run.id.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(runs[0].id.to_string(), run_id);

    common::shutdown_gracefully(&mut service.child).await;
}

/// A linked trigger request reads terminal once its run reaches a terminal
/// verdict. The trigger is submitted for real, and the real consumer plans
/// the run and links it into the durable request before the run's workspace
/// preparation begins. Preparation is then made deterministically impossible
/// — the configured workspace root sits beneath a regular file — so the
/// service's own planning-failure path records the terminal verdict on the
/// run row and closes the linked request with the cause. Nothing here writes
/// a status: the endpoint must report the run's durable verdict under the
/// operator credential, with the linked run id still in place.
#[tokio::test]
async fn test_linked_trigger_request_reads_terminal_after_the_run_verdict() {
    // A regular file occupying the workspace root's would-be parent: the
    // service cannot create its workspace root there, so the run planned for
    // the trigger below fails for a real cause inside the real service.
    let blocker_root =
        std::env::temp_dir().join(format!("gitforge-ci-ws-blocker-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&blocker_root).expect("create blocker directory");
    let blocker = blocker_root.join("not-a-directory");
    std::fs::write(&blocker, "a regular file blocks workspace creation").expect("write blocker");
    let workspace_root = blocker.join("workspaces");

    let mut service = spawn_ci_with(
        &[
            (
                "GITFORGE_SCHEDULER_OPERATOR_TOKEN",
                "harness-operator-token",
            ),
            (
                "GITFORGE_WORKSPACE_ROOT",
                workspace_root.to_str().expect("utf-8 blocker path"),
            ),
        ],
        true,
    )
    .await;

    let trigger_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.repo_id,
        "0".repeat(40),
        service.commit_hash
    );
    let operator = "Bearer harness-operator-token";

    // The trigger credential submits; the consumer plans the run and links it
    // into the durable request before the run's workspace preparation fails.
    let (status, body) = post_trigger_with_header(
        service.scheduler_port,
        Some(("Authorization", "Bearer harness-trigger-token")),
        &trigger_body,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("parse trigger response");
    let trigger_id = payload["trigger_id"]
        .as_str()
        .expect("trigger_id in response")
        .to_string();
    assert!(
        uuid::Uuid::parse_str(&trigger_id).is_ok(),
        "trigger_id must be a UUID: {body}"
    );

    // The linked request reads terminal once the run's verdict is durable.
    let mut lifecycle = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() >= deadline {
            panic!("linked trigger request never read terminal: {lifecycle}");
        }
        let (status, body) =
            get_trigger_request(service.scheduler_port, &trigger_id, Some(operator)).await;
        assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("parse lifecycle");
        if parsed["status"] == "failed" {
            lifecycle = body;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let terminal: serde_json::Value = serde_json::from_str(&lifecycle).expect("parse lifecycle");
    let run_id = terminal["pipeline_run_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .expect("the terminal request stays linked to its run")
        .to_string();
    assert!(
        terminal["error"]
            .as_str()
            .is_some_and(|cause| !cause.is_empty()),
        "the terminal request carries the run's real cause: {lifecycle}"
    );
    assert!(
        !lifecycle.contains("harness-trigger-token")
            && !lifecycle.contains("harness-operator-token"),
        "no response may echo a credential: {lifecycle}"
    );

    // The verdict the endpoint reported is the run's own durable verdict,
    // which this service wrote when planning failed.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert_eq!(runs.len(), 1, "expected exactly one run");
    assert_eq!(runs[0].id.to_string(), run_id);
    assert_eq!(
        runs[0].status, "failed",
        "the linked run reached a terminal verdict"
    );
    assert!(
        runs[0].error.is_some(),
        "the durable run carries its failure cause"
    );

    common::shutdown_gracefully(&mut service.child).await;
}

/// A linked trigger request stays deduplicable no matter how long its
/// preparation takes: the correlation window bounds only the unlinked span
/// between recording the request and the consumer linking its durably created
/// run. A workspace clone that outlives the 15-second window must therefore
/// never turn a repeat POST into a second run for the same commit. The
/// window's passage is simulated deterministically by backdating the linked
/// request's row instead of sleeping through a real slow clone.
#[tokio::test]
async fn test_repeat_trigger_stays_deduplicated_past_the_correlation_window() {
    let mut service = spawn_ci_with(
        &[(
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN",
            "harness-operator-token",
        )],
        true,
    )
    .await;

    let trigger_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.repo_id,
        "0".repeat(40),
        service.commit_hash
    );
    let operator = "Bearer harness-operator-token";

    let (status, body) = post_trigger_with_header(
        service.scheduler_port,
        Some(("Authorization", "Bearer harness-trigger-token")),
        &trigger_body,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("parse trigger response");
    let trigger_id = payload["trigger_id"]
        .as_str()
        .expect("trigger_id in response")
        .to_string();

    // The consumer planned the run and linked it into the request.
    let mut linked: Option<serde_json::Value> = None;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() >= deadline {
            panic!("trigger request never reached processing");
        }
        let (status, body) =
            get_trigger_request(service.scheduler_port, &trigger_id, Some(operator)).await;
        assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("parse lifecycle");
        if parsed["status"] == "processing" && parsed["pipeline_run_id"].is_string() {
            linked = Some(parsed);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let lifecycle = linked.expect("trigger request reached processing");

    // The correlation window elapses while preparation is still going. The
    // link is already in place, so the request must not fall back out of the
    // dedup — least of all be swept closed — on account of its age alone.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let backdated = (chrono::Utc::now()
        - chrono::Duration::seconds(
            i64::try_from(gitforge_common::CI_TRIGGER_CORRELATION_WINDOW.as_secs())
                .expect("window fits i64"),
        )
        - chrono::Duration::hours(1))
    .to_rfc3339();
    sqlx::query("UPDATE ci_trigger_requests SET created_at = ? WHERE id = ?")
        .bind(&backdated)
        .bind(&trigger_id)
        .execute(pool.pool())
        .await
        .expect("backdate linked trigger request");

    // A repeat POST minutes into the clone still collapses into the same
    // request instead of planning a second run for the same commit.
    let (status, body) = post_trigger_with_header(
        service.scheduler_port,
        Some(("Authorization", "Bearer harness-trigger-token")),
        &trigger_body,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let repeat: serde_json::Value = serde_json::from_str(&body).expect("parse repeat response");
    assert_eq!(repeat["status"], "deduplicated", "body: {body}");
    assert_eq!(repeat["trigger_id"], payload["trigger_id"], "body: {body}");

    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert_eq!(
        runs.len(),
        1,
        "a repeat POST past the window must not create a second run: {:?}",
        runs.iter()
            .map(|run| run.id.to_string())
            .collect::<Vec<_>>()
    );
    let linked_run = lifecycle["pipeline_run_id"]
        .as_str()
        .expect("linked run id")
        .to_string();
    assert_eq!(runs[0].id.to_string(), linked_run);

    common::shutdown_gracefully(&mut service.child).await;
}

/// Without a durable store there is no lifecycle to report, so the endpoint
/// says so instead of pretending correlation exists.
#[tokio::test]
async fn test_trigger_request_status_reports_a_missing_store() {
    let mut service = spawn_ci_with(
        &[(
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN",
            "harness-operator-token",
        )],
        false,
    )
    .await;

    let (status, body) = get_trigger_request(
        service.scheduler_port,
        &uuid::Uuid::new_v4().to_string(),
        Some("Bearer harness-operator-token"),
    )
    .await;
    assert_eq!(
        status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "body: {body}"
    );
    assert!(body.contains("trigger_store_unavailable"), "body: {body}");

    common::shutdown_gracefully(&mut service.child).await;
}

/// With no operator credential configured the endpoint is closed outright,
/// even though a trigger credential is set: the read path never falls back to
/// the submit credential, so presenting the submit credential on
/// `Authorization` still gets the unconfigured-read-auth refusal.
#[tokio::test]
async fn test_trigger_request_status_requires_the_operator_credential() {
    let mut service = spawn_ci_with(&[], false).await;

    let read_id = uuid::Uuid::new_v4().to_string();
    let (status, body) = get_trigger_request(service.scheduler_port, &read_id, None).await;
    assert_eq!(
        status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "body: {body}"
    );
    assert!(
        body.contains("trigger_request_auth_not_configured"),
        "body: {body}"
    );

    // The submit credential is presented and still refused: with no read
    // credential configured the endpoint fails closed for every caller, so a
    // leaked trigger token buys no visibility here either.
    let (status, body) = get_trigger_request(
        service.scheduler_port,
        &read_id,
        Some("Bearer harness-trigger-token"),
    )
    .await;
    assert_eq!(
        status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "the submit credential must not open the unconfigured read path: {body}"
    );
    assert!(
        body.contains("trigger_request_auth_not_configured"),
        "body: {body}"
    );

    common::shutdown_gracefully(&mut service.child).await;
}

/// One secret configured for both the submit and the read role has no
/// separation left to enforce, so both endpoints fail closed with the same
/// generic misconfiguration response instead of honoring either role with it.
#[tokio::test]
async fn test_identical_submit_and_read_credentials_fail_closed() {
    let mut service = spawn_ci_with(
        &[
            ("GITFORGE_TRIGGER_TOKEN", "harness-shared-token"),
            ("GITFORGE_SCHEDULER_OPERATOR_TOKEN", "harness-shared-token"),
        ],
        true,
    )
    .await;

    let trigger_body = format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"working_dir\":null}}",
        service.repo_id,
        "0".repeat(40),
        service.commit_hash
    );

    // The submit endpoint refuses the shared secret on either header form.
    for header in [
        ("x-gitforge-trigger-token", "harness-shared-token"),
        ("Authorization", "Bearer harness-shared-token"),
    ] {
        let (status, body) =
            post_trigger_with_header(service.scheduler_port, Some(header), &trigger_body).await;
        assert_eq!(
            status,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "body: {body}"
        );
        assert!(body.contains("trigger_auth_misconfigured"), "body: {body}");
        assert!(
            !body.contains("harness-shared-token"),
            "no response may echo a credential: {body}"
        );
    }

    // The read endpoint fails closed on the shared secret too.
    let (status, body) = get_trigger_request(
        service.scheduler_port,
        &uuid::Uuid::new_v4().to_string(),
        Some("Bearer harness-shared-token"),
    )
    .await;
    assert_eq!(
        status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "body: {body}"
    );
    assert!(body.contains("trigger_auth_misconfigured"), "body: {body}");

    // Fail-closed means nothing was ever planned for this deployment.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert!(
        runs.is_empty(),
        "a collided deployment must plan no runs: {:?}",
        runs.iter()
            .map(|run| run.id.to_string())
            .collect::<Vec<_>>()
    );

    common::shutdown_gracefully(&mut service.child).await;
}

/// A claim the previous process took but never resolved must be closed by the
/// next boot: its event lived in the old process's in-memory bus, which died
/// with that process, so no consumer will ever link a run or record a failure
/// for it. The boot sweep closes exactly those rows, leaves an
/// already-terminal verdict untouched, closes an ancient `pending` row only
/// through the stale sweep with its own distinct cause, and the fresh process
/// keeps accepting and planning real triggers.
#[tokio::test]
async fn test_restart_sweep_closes_claims_lost_to_the_previous_process() {
    let hour_ago = chrono::Utc::now() - chrono::Duration::hours(1);
    let abandoned_event = uuid::Uuid::new_v4();
    let stale_event = uuid::Uuid::new_v4();
    let terminal_event = uuid::Uuid::new_v4();
    let mut service = spawn_ci_seeded(
        &[(
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN",
            "harness-operator-token",
        )],
        &[
            // Claimed by the previous process, never linked: the boot sweep's
            // to close.
            SeedTriggerRequest {
                event_id: abandoned_event,
                status: "claimed",
                created_at: hour_ago,
                error: None,
            },
            // Pending and ancient: the stale sweep closes it, with its own
            // distinct cause.
            SeedTriggerRequest {
                event_id: stale_event,
                status: "pending",
                created_at: hour_ago,
                error: None,
            },
            // Already terminal: no sweep may rewrite the verdict.
            SeedTriggerRequest {
                event_id: terminal_event,
                status: "failed",
                created_at: hour_ago,
                error: Some("planning failed before the previous process stopped"),
            },
        ],
    )
    .await;
    let operator = "Bearer harness-operator-token";

    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let abandoned_id = seeded_trigger_id(&pool, abandoned_event).await;
    let stale_id = seeded_trigger_id(&pool, stale_event).await;
    let terminal_id = seeded_trigger_id(&pool, terminal_event).await;

    // The sweeps run in the service's startup task, which can still be in
    // flight when the health endpoint first answers: poll for the closed
    // verdicts instead of assuming they have landed.
    let mut abandoned_lifecycle = String::new();
    let mut stale_lifecycle = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() >= deadline {
            panic!(
                "boot sweeps never closed the seeded rows: abandoned={abandoned_lifecycle} \
                 stale={stale_lifecycle}"
            );
        }
        let (status, body) =
            get_trigger_request(service.scheduler_port, &abandoned_id, Some(operator)).await;
        assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("parse lifecycle");
        if parsed["status"] == "failed" {
            abandoned_lifecycle = body;
        }
        let (status, body) =
            get_trigger_request(service.scheduler_port, &stale_id, Some(operator)).await;
        assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
        let stale_parsed: serde_json::Value = serde_json::from_str(&body).expect("parse lifecycle");
        if stale_parsed["status"] == "failed" {
            stale_lifecycle = body;
        }
        if !abandoned_lifecycle.is_empty() && !stale_lifecycle.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // The lost claim carries the restart cause and never planned a run.
    let abandoned_payload: serde_json::Value =
        serde_json::from_str(&abandoned_lifecycle).expect("parse abandoned lifecycle");
    assert!(
        abandoned_payload["error"]
            .as_str()
            .is_some_and(|cause| cause.contains("restart")),
        "the cause must name the restart: {abandoned_lifecycle}"
    );
    assert!(
        abandoned_payload["pipeline_run_id"].is_null(),
        "a lost claim never planned a run: {abandoned_lifecycle}"
    );

    // The stale-pending row is closed by the stale sweep, not the restart
    // sweep: the two causes must stay distinguishable.
    let stale_payload: serde_json::Value =
        serde_json::from_str(&stale_lifecycle).expect("parse stale lifecycle");
    assert!(
        stale_payload["error"]
            .as_str()
            .is_some_and(|cause| cause.contains("correlation window")),
        "a stale pending row is closed by the window sweep: {stale_lifecycle}"
    );

    // The terminal verdict recorded before the restart is untouched.
    let (status, body) =
        get_trigger_request(service.scheduler_port, &terminal_id, Some(operator)).await;
    assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
    let terminal_payload: serde_json::Value =
        serde_json::from_str(&body).expect("parse terminal lifecycle");
    assert_eq!(terminal_payload["status"], "failed", "body: {body}");
    assert_eq!(
        terminal_payload["error"], "planning failed before the previous process stopped",
        "no sweep may rewrite a terminal verdict: {body}"
    );

    // The fresh process still accepts and plans real work end to end.
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
    assert_eq!(payload["status"], "accepted", "body: {body}");
    assert!(
        payload["pipeline_run_id"].is_string(),
        "the post-restart trigger plans a run: {body}"
    );

    common::shutdown_gracefully(&mut service.child).await;
}

/// A queued trigger request that is still inside the correlation window when
/// the process comes up must not sit `pending` forever: the running service's
/// periodic sweep closes it once the window elapses, and the status endpoint
/// reports the expiry the service itself wrote. Unlike the restart test above
/// — whose stale row is ancient at boot and is closed by the startup pass —
/// this row is fresh when the service boots, so only a sweep pass that runs
/// *after* the process is serving can close it. The fixture writes the row as
/// `pending` because that is the only deterministic way a row stays queued: a
/// real POST is claimed by the live consumer within the window and resolves
/// through its run. The terminal verdict is never fabricated — the test only
/// asserts what the service wrote to the durable store.
#[tokio::test]
async fn test_queued_trigger_request_expires_after_the_correlation_window_while_running() {
    // Fresh at seed time: inside the 15-second correlation window
    // (`gitforge_common::CI_TRIGGER_CORRELATION_WINDOW`) when the sweeps
    // first observe it.
    let mut service = spawn_ci_with(
        &[(
            "GITFORGE_SCHEDULER_OPERATOR_TOKEN",
            "harness-operator-token",
        )],
        true,
    )
    .await;
    let operator = "Bearer harness-operator-token";

    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let queued_event = uuid::Uuid::new_v4();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(TRIGGER_REQUEST_STORE_DDL)
        .execute(pool.pool())
        .await
        .expect("ensure trigger-request table");
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_ci_trigger_requests_event_id
         ON ci_trigger_requests (event_id)",
    )
    .execute(pool.pool())
    .await
    .expect("ensure trigger-request event index");
    sqlx::query(
        "INSERT INTO ci_trigger_requests
         (id, event_id, repo_id, ref_name, new_hash, status, pipeline_run_id, error,
          created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'pending', NULL, NULL, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(queued_event.to_string())
    .bind(service.repo_id.to_string())
    .bind("refs/heads/main")
    .bind(&service.commit_hash)
    .bind(&now)
    .bind(&now)
    .execute(pool.pool())
    .await
    .expect("insert queued request after service startup");
    let queued_id = seeded_trigger_id(&pool, queued_event).await;

    // The service is already serving before this fresh pending row exists,
    // which rules out the boot sweep as the path that closes it.
    let (status, body) =
        get_trigger_request(service.scheduler_port, &queued_id, Some(operator)).await;
    assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
    let lifecycle: serde_json::Value = serde_json::from_str(&body).expect("parse lifecycle");
    assert_eq!(lifecycle["status"], "pending", "body: {body}");
    assert!(lifecycle["pipeline_run_id"].is_null(), "body: {body}");

    // Wait out the correlation window plus the periodic sweep's 60-second
    // tick (`RECONCILE_INTERVAL_SECS` in the service), polling the real
    // status endpoint for the expiry verdict the service writes. Bounded
    // wait, never a sleep-and-hope.
    let deadline = Instant::now() + Duration::from_secs(120);
    let expired = loop {
        if Instant::now() >= deadline {
            panic!("the queued request never expired through the status endpoint: {body}");
        }
        let (status, body) =
            get_trigger_request(service.scheduler_port, &queued_id, Some(operator)).await;
        assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("parse lifecycle");
        if parsed["status"] == "failed" {
            break body;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };

    // The service wrote the expiry: the correlation-window cause, no run
    // linked, and the verdict is durable in the store the service owns.
    let payload: serde_json::Value = serde_json::from_str(&expired).expect("parse expiry");
    assert_eq!(
        payload["error"], "trigger event was never planned within the correlation window",
        "the expiry must carry the correlation-window cause: {expired}"
    );
    assert!(
        payload["pipeline_run_id"].is_null(),
        "an expired queued request never planned a run: {expired}"
    );

    use sqlx::Row;
    let row =
        sqlx::query("SELECT status, error, pipeline_run_id FROM ci_trigger_requests WHERE id = ?")
            .bind(&queued_id)
            .fetch_one(pool.pool())
            .await
            .expect("durable trigger-request row");
    assert_eq!(
        row.try_get::<String, _>("status").expect("status"),
        "failed",
        "the expiry must be durable, not a read-time grade"
    );
    assert!(
        row.try_get::<Option<String>, _>("pipeline_run_id")
            .expect("run id")
            .is_none(),
        "the durable row must stay unlinked"
    );

    // Expiry is not planning: no run may exist for this request's push.
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert!(
        runs.is_empty(),
        "an expired queued request must plan no runs: {:?}",
        runs.iter()
            .map(|run| run.id.to_string())
            .collect::<Vec<_>>()
    );

    common::shutdown_gracefully(&mut service.child).await;
}
