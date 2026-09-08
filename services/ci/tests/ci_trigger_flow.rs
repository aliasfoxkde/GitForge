//! End-to-end CI trigger flow test.
//!
//! This test spawns the real `ci` binary against a temporary SQLite
//! database, bare git repository, workspace root, and artifact root, then
//! drives the same HTTP trigger endpoint the git-server calls after a
//! successful push. Nothing is mocked: the trigger auth middleware, the
//! in-process event consumer, the committed `.gitforce.yml` loader, the
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

/// Prepare the database, bare repository with a committed `.gitforce.yml`,
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
    std::fs::write(seed.join(".gitforce.yml"), COMMITTED_PIPELINE).expect("write pipeline");
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
        std::fs::read_to_string(workspace.join(".gitforce.yml")).expect("workspace pipeline file");
    assert_eq!(checked_out, COMMITTED_PIPELINE);

    common::shutdown_gracefully(&mut service.child).await;
}
