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

/// A committed but unparseable definition: `version` is required, so this
/// fails `PipelineDefinition::parse` and the trigger must grade the event
/// failed instead of silently running a substituted pipeline.
const BROKEN_PIPELINE: &str = r#"
name: broken-pipeline
trigger_on:
  - push
jobs: []
"#;

const TRIGGER_TOKEN: &str = "harness-trigger-token";
const STATUS_TOKEN: &str = "harness-status-token";

/// Environment for a spawned ci service.
struct CiService {
    child: tokio::process::Child,
    scheduler_port: u16,
    db_path: PathBuf,
    workspace_root: PathBuf,
    artifact_root: PathBuf,
    seed_dir: PathBuf,
    repo_id: RepoId,
    user_id: UserId,
    commit_hash: String,
}

impl CiService {
    /// Restart the service against the same durable state: the current
    /// process exits gracefully and a fresh process opens the same SQLite
    /// file on a new port. This is the seam the trigger correlation contract
    /// must survive (issue #259).
    async fn respawn(mut self) -> CiService {
        common::shutdown_gracefully(&mut self.child).await;
        let (child, scheduler_port) =
            spawn_ci_process(&self.db_path, &self.artifact_root, &self.workspace_root).await;
        self.child = child;
        self.scheduler_port = scheduler_port;
        self
    }
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

/// Spawn one ci binary against an existing layout and wait for its health
/// endpoint. Split from [`spawn_ci`] so a service can be restarted against
/// the same database.
async fn spawn_ci_process(
    db_path: &Path,
    artifacts: &Path,
    workspaces: &Path,
) -> (tokio::process::Child, u16) {
    let scheduler_port = free_port();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ci"))
        .env("SCHEDULER_PORT", scheduler_port.to_string())
        .env(
            "GITFORGE_DATABASE_URL",
            format!("sqlite:{}", db_path.display()),
        )
        .env("GITFORGE_TRIGGER_TOKEN", TRIGGER_TOKEN)
        .env("GITFORGE_STATUS_TOKEN", STATUS_TOKEN)
        .env("GITFORGE_ARTIFACT_ROOT", artifacts)
        .env("GITFORGE_WORKSPACE_ROOT", workspaces)
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

    (child, scheduler_port)
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
    // carrying the committed pipeline definition. The seed checkout stays
    // around so tests can push follow-up commits (e.g. a broken definition).
    let bare = git_root.join("harness.git");
    std::fs::create_dir_all(&bare).expect("create bare dir");
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

    let (child, scheduler_port) = spawn_ci_process(&db_path, &artifacts, &workspaces).await;

    CiService {
        child,
        scheduler_port,
        db_path,
        workspace_root: workspaces,
        artifact_root: artifacts,
        seed_dir: seed,
        repo_id,
        user_id,
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

/// GET `/pipelines/trigger/status/{event_id}`. `auth` selects the credential:
/// `None` sends nothing, `(header, value)` sends one header.
async fn get_trigger_status(
    port: u16,
    auth: Option<(&'static str, String)>,
    event_id: &str,
) -> (reqwest::StatusCode, String) {
    let url = format!("http://127.0.0.1:{port}/pipelines/trigger/status/{event_id}");
    let mut request = reqwest::Client::new().get(&url);
    if let Some((header, value)) = auth {
        request = request.header(header, value);
    }
    let response = request.send().await.expect("send status request");
    let status = response.status();
    let text = response.text().await.expect("read status body");
    (status, text)
}

fn status_bearer() -> (&'static str, String) {
    ("Authorization", format!("Bearer {STATUS_TOKEN}"))
}

/// Poll the status endpoint until `probe` accepts a 200 body, returning the
/// accepted body. Mirrors what the GitHub Actions poll job does, bounded so a
/// regression fails the test instead of hanging it.
async fn poll_trigger_status<F>(port: u16, event_id: &str, mut probe: F) -> serde_json::Value
where
    F: FnMut(&serde_json::Value) -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (status, body) = get_trigger_status(port, Some(status_bearer()), event_id).await;
        assert_eq!(
            status,
            reqwest::StatusCode::OK,
            "status endpoint answered {status}: {body}"
        );
        let payload: serde_json::Value = serde_json::from_str(&body).expect("parse status body");
        if probe(&payload) || tokio::time::Instant::now() >= deadline {
            return payload;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn trigger_body(service: &CiService, new_hash: &str) -> String {
    format!(
        "{{\"repo_id\":\"{}\",\"ref_name\":\"refs/heads/main\",\
          \"old_hash\":\"{}\",\"new_hash\":\"{}\",\"pusher_id\":\"{}\",\
          \"working_dir\":null}}",
        service.repo_id, service.commit_hash, new_hash, service.user_id
    )
}

#[tokio::test]
async fn test_trigger_requires_token_and_runs_committed_pipeline() {
    let mut service = spawn_ci().await;

    let trigger_body = trigger_body(&service, &service.commit_hash);

    // Without the trigger token the endpoint must not start anything.
    let (status, body) = post_trigger(service.scheduler_port, None, &trigger_body).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "body: {body}");
    assert!(body.contains("trigger_auth_required"), "body: {body}");

    // With the token, the push event flows through the real in-process
    // consumer: the committed pipeline governs, the run is created, and its
    // id is reported synchronously.
    let (status, body) =
        post_trigger(service.scheduler_port, Some(TRIGGER_TOKEN), &trigger_body).await;
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

/// The status endpoint is a separate credential domain (issue #259): no
/// credential, the trigger token, and a wrong status token are all
/// rejected; only the dedicated status token reads the correlation. The
/// response is limited to the contract's three fields.
#[tokio::test]
async fn test_status_endpoint_requires_the_dedicated_status_credential() {
    let mut service = spawn_ci().await;

    let (status, body) = post_trigger(
        service.scheduler_port,
        Some(TRIGGER_TOKEN),
        &trigger_body(&service, &service.commit_hash),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let trigger: serde_json::Value = serde_json::from_str(&body).expect("parse trigger response");
    let event_id = trigger["event_id"].as_str().expect("event_id").to_string();
    let run_id = trigger["pipeline_run_id"].as_str().expect("run id");

    // No credential at all.
    let (status, body) = get_trigger_status(service.scheduler_port, None, &event_id).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "body: {body}");
    assert!(body.contains("status_auth_required"), "body: {body}");

    // The trigger credential must not satisfy the status endpoint: the two
    // credentials are scoped separately, and neither substitutes for the
    // other in either direction.
    let (status, body) = get_trigger_status(
        service.scheduler_port,
        Some(("Authorization", format!("Bearer {TRIGGER_TOKEN}"))),
        &event_id,
    )
    .await;
    assert_eq!(
        status,
        reqwest::StatusCode::UNAUTHORIZED,
        "the trigger token must not read status: {body}"
    );

    // A wrong status token is rejected too, in both header forms.
    let (status, body) = get_trigger_status(
        service.scheduler_port,
        Some(("x-gitforge-status-token", "not-the-token".to_string())),
        &event_id,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "body: {body}");

    // The dedicated status token reads the correlation, and the correlated
    // run id is exactly the one the triggering call reported.
    let payload = poll_trigger_status(service.scheduler_port, &event_id, |payload| {
        payload["pipeline_run_id"].is_string()
    })
    .await;
    assert_eq!(payload["event_id"], event_id.as_str());
    assert_eq!(payload["pipeline_run_id"], run_id);
    // The run is live (job enqueued, no runner attached): non-terminal.
    assert_eq!(payload["status"], "running");

    // The contract exposes only the correlation and the lifecycle state.
    let payload = poll_trigger_status(service.scheduler_port, &event_id, |_| true).await;
    let fields = payload.as_object().expect("status object");
    assert_eq!(fields.len(), 3, "unexpected fields: {fields:?}");
    for expected in ["event_id", "pipeline_run_id", "status"] {
        assert!(
            fields.contains_key(expected),
            "missing contract field {expected}"
        );
    }

    common::shutdown_gracefully(&mut service.child).await;
}

/// A `queued` trigger answer must stay resolvable by event id across a
/// service restart (issue #259): the correlation is durable, not in-memory.
#[tokio::test]
async fn test_event_correlation_survives_service_restart() {
    let service = spawn_ci().await;

    let (status, body) = post_trigger(
        service.scheduler_port,
        Some(TRIGGER_TOKEN),
        &trigger_body(&service, &service.commit_hash),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let trigger: serde_json::Value = serde_json::from_str(&body).expect("parse trigger response");
    let event_id = trigger["event_id"].as_str().expect("event_id").to_string();
    let run_id = trigger["pipeline_run_id"]
        .as_str()
        .expect("run id")
        .to_string();

    // Before the restart: the event correlates to the reported run.
    let payload = poll_trigger_status(service.scheduler_port, &event_id, |payload| {
        payload["pipeline_run_id"].is_string()
    })
    .await;
    assert_eq!(payload["pipeline_run_id"], run_id.as_str());
    assert_eq!(payload["status"], "running");

    // Restart the process against the same database on a new port.
    let mut service = service.respawn().await;

    // After the restart: still correlated, from the durable rows.
    let (status, body) =
        get_trigger_status(service.scheduler_port, Some(status_bearer()), &event_id).await;
    assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("parse status body");
    assert_eq!(payload["pipeline_run_id"], run_id.as_str());
    assert!(
        payload["status"] == "running" || payload["status"] == "queued",
        "restarted status must be non-terminal: {payload}"
    );

    // And the correlation names a run that really exists durably.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id.to_string(), run_id);

    common::shutdown_gracefully(&mut service.child).await;
}

/// An event id the service never issued is `missing` (404), and a malformed
/// one is a bad request — neither is ever a green answer.
#[tokio::test]
async fn test_unknown_or_malformed_event_id_is_never_green() {
    let mut service = spawn_ci().await;

    let unknown = uuid::Uuid::new_v4().to_string();
    let (status, body) =
        get_trigger_status(service.scheduler_port, Some(status_bearer()), &unknown).await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("parse 404 body");
    assert_eq!(payload["status"], "missing");
    assert_eq!(payload["error"], "unknown_event_id");

    let (status, body) =
        get_trigger_status(service.scheduler_port, Some(status_bearer()), "not-a-uuid").await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "body: {body}");
    assert!(body.contains("invalid_event_id"), "body: {body}");

    common::shutdown_gracefully(&mut service.child).await;
}

/// A committed but unparseable pipeline definition is a trigger error: the
/// consumer records the event failed, the status endpoint answers the
/// terminal `failed`, and no run row is ever created for the broken commit.
#[tokio::test]
async fn test_unparseable_pipeline_marks_trigger_failed() {
    let mut service = spawn_ci().await;

    std::fs::write(service.seed_dir.join(".gitforge.yml"), BROKEN_PIPELINE)
        .expect("write broken pipeline");
    run_git(&["add", "."], &service.seed_dir);
    run_git(
        &["commit", "-m", "commit an unparseable definition"],
        &service.seed_dir,
    );
    run_git(&["push", "origin", "main"], &service.seed_dir);
    let broken_hash = run_git(&["rev-parse", "HEAD"], &service.seed_dir);

    let (status, body) = post_trigger(
        service.scheduler_port,
        Some(TRIGGER_TOKEN),
        &trigger_body(&service, &broken_hash),
    )
    .await;
    // The consumer fails before creating a run, so the synchronous
    // correlation window elapses unanswered: `queued` with an event id only.
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "body: {body}");
    let trigger: serde_json::Value = serde_json::from_str(&body).expect("parse trigger response");
    assert_eq!(trigger["status"], "queued", "body: {body}");
    assert!(trigger["pipeline_run_id"].is_null(), "body: {body}");
    let event_id = trigger["event_id"].as_str().expect("event_id").to_string();

    // The durable correlation reaches the terminal failure, never green.
    let payload = poll_trigger_status(service.scheduler_port, &event_id, |payload| {
        payload["status"] == "failed"
    })
    .await;
    assert_eq!(payload["status"], "failed");
    assert!(payload["pipeline_run_id"].is_null());

    // No run row exists for the broken commit.
    let pool = gitforge_db::Pool::new(&service.db_path.display().to_string())
        .await
        .expect("reopen service database");
    let runs = gitforge_db::queries::PipelineRunQueries::list(&pool)
        .await
        .expect("list runs");
    assert!(runs.is_empty(), "a failed trigger must not create runs");

    common::shutdown_gracefully(&mut service.child).await;
}
