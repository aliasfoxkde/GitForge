//! Ref-update policy enforcement over the wire (#240).
//!
//! `ref_policy.rs` carries its own unit tests for the decision logic; this
//! suite covers what only a real server can prove: that a repository with
//! required checks declines a branch advance with a standard receive-pack
//! rejection report the `git` client renders as `! [remote rejected]`, that
//! the declined ref never advances in the bare repository, that a green
//! pipeline run for the commit lets the same push through, that tag pushes
//! bypass the gate, and that branch deletions are accepted without minting
//! a CI trigger.

// Test-harness exemption, same discipline as the sibling git-server suites:
// `allow-unwrap-in-tests` covers `#[test]` bodies, but boot/seed/fixture
// helper functions in a test target are neither `#[test]` fns nor
// `#[cfg(test)]`, a class the clippy.toml config cannot address. Setup
// failing IS the assertion -- a panic aborts the run loudly. Production
// code keeps the denies.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::{Command, Stdio};
use std::time::Duration;

use gitforge_common::{RepoId, UserId};

mod common;

use common::{free_port, run_git};

/// Environment for a spawned git-server instance.
struct TestServer {
    child: tokio::process::Child,
    http_port: u16,
    repo_id: RepoId,
    db_path: std::path::PathBuf,
    base: std::path::PathBuf,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Run a `git` command and return its output without asserting on the
/// exit status — for pushes that are expected to be rejected and for
/// probing refs that may not exist.
fn try_git(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .expect("spawn git")
}

/// Spawn the real git-server binary over a seeded `testowner/proto`
/// repository whose ref-update policy is preconfigured by `seed_database`.
async fn spawn_server(required_checks: &[&str]) -> TestServer {
    let unique = uuid::Uuid::new_v4();
    let base = std::env::temp_dir().join(format!("git-server-policy-{unique}"));
    let git_root = base.join("git");
    let db_path = base.join("gitforge.db");
    std::fs::create_dir_all(&git_root).expect("create git root");

    let repo_id = seed_database(&db_path, &git_root, required_checks).await;

    let http_port = free_port();
    let ssh_port = free_port();
    // No GITFORGE_CI_TRIGGER_URL: delivery is a no-op and enqueued trigger
    // rows stay pending, which the assertions read directly.
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_git-server"))
        .env("HTTP_PORT", http_port.to_string())
        .env("SSH_PORT", ssh_port.to_string())
        .env("GIT_ROOT", &git_root)
        .env("DATABASE_URL", format!("sqlite:{}", db_path.display()))
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn git-server binary");
    let child = wait_healthy(child, http_port).await;

    TestServer {
        child,
        http_port,
        repo_id,
        db_path,
        base,
    }
}

/// Seed a database with `testowner/proto`, create its bare repository under
/// the git root, and configure the repository's ref-update policy.
async fn seed_database(
    db_path: &std::path::Path,
    git_root: &std::path::Path,
    required_checks: &[&str],
) -> RepoId {
    let pool = gitforge_db::Pool::new(&db_path.display().to_string())
        .await
        .expect("create sqlite pool");
    pool.migrate().await.expect("run migrations");
    let user = gitforge_db::models::User::new(
        "testowner".to_string(),
        "testowner@example.com".to_string(),
        "hash".to_string(),
    );
    let user_id: UserId = user.id;
    gitforge_db::queries::UserQueries::create(&pool, &user)
        .await
        .expect("create user");
    let repository =
        gitforge_db::models::Repository::new("proto".to_string(), user_id, "git".to_string());
    let repo_id = repository.id;
    gitforge_db::queries::RepoQueries::create(&pool, &repository)
        .await
        .expect("create repository");
    if !required_checks.is_empty() {
        let checks: Vec<String> = required_checks
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        gitforge_db::queries::RepoQueries::update_policy(&pool, repo_id, &checks, true)
            .await
            .expect("configure ref policy");
    }
    drop(pool);

    let bare_repo = git_root.join(repo_id.to_string());
    run_git(
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            bare_repo.to_str().unwrap(),
        ],
        git_root,
        &[],
    );
    repo_id
}

async fn wait_healthy(mut child: tokio::process::Child, http_port: u16) -> tokio::process::Child {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let health_url = format!("http://127.0.0.1:{http_port}/health");
    loop {
        if tokio::time::Instant::now() >= deadline {
            let _ = child.start_kill();
            panic!("git-server did not become healthy at {health_url}");
        }
        if let Ok(response) = client.get(&health_url).send().await {
            if response.status().is_success() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    child
}

/// A commit pushed through the real `git` client; returns (sha, work dir).
/// The caller decides whether the push may fail, so the push itself uses
/// `try_git` and the assertion is the test's.
fn make_commit(server: &TestServer, name: &str) -> (String, std::path::PathBuf) {
    let work = server.base.join(format!("work-{name}"));
    std::fs::create_dir_all(&work).expect("create work dir");
    run_git(&["init", "--initial-branch=main"], &work, &[]);
    run_git(&["config", "user.email", "dev@example.com"], &work, &[]);
    run_git(&["config", "user.name", "Policy Test"], &work, &[]);
    std::fs::write(work.join(format!("{name}.txt")), format!("{name}\n")).expect("write file");
    run_git(&["add", "."], &work, &[]);
    run_git(&["commit", "-m", name], &work, &[]);
    run_git(
        &[
            "remote",
            "add",
            "origin",
            &format!("http://127.0.0.1:{}/testowner/proto", server.http_port),
        ],
        &work,
        &[],
    );
    let head = run_git(&["rev-parse", "HEAD"], &work, &[]);
    (
        String::from_utf8_lossy(&head.stdout).trim().to_string(),
        work,
    )
}

/// Bare repository backing the seeded repo — the source of truth for what
/// actually advanced.
fn bare_repo(server: &TestServer) -> std::path::PathBuf {
    server.base.join("git").join(server.repo_id.to_string())
}

async fn open_db(server: &TestServer) -> gitforge_db::Pool {
    gitforge_db::Pool::new(&server.db_path.display().to_string())
        .await
        .expect("open db for assertions")
}

/// Insert a green run of the `ci` pipeline for `commit` so a gated push of
/// that commit passes the required-checks evaluation.
async fn seed_green_run(server: &TestServer, commit: &str) {
    let pool = open_db(server).await;
    let pipeline = gitforge_db::models::Pipeline {
        id: gitforge_common::PipelineId::new(),
        repo_id: server.repo_id,
        name: "ci".to_string(),
        trigger_type: "push".to_string(),
        config: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    gitforge_db::queries::PipelineQueries::create(&pool, &pipeline)
        .await
        .expect("create ci pipeline");
    let run = gitforge_db::models::PipelineRun::new(
        pipeline.id,
        server.repo_id,
        "push".to_string(),
        commit.to_string(),
    );
    gitforge_db::queries::PipelineRunQueries::create(&pool, &run)
        .await
        .expect("create pipeline run");
    gitforge_db::queries::PipelineRunQueries::update_status(&pool, run.id, "succeeded")
        .await
        .expect("grade run succeeded");
}

/// All `ci.trigger.*` outbox rows as (ref_name, new_hash) pairs.
async fn trigger_rows(server: &TestServer) -> Vec<(String, String)> {
    let pool = open_db(server).await;
    sqlx::query_as::<_, (String, String)>(
        "SELECT event_type, payload FROM events WHERE event_type LIKE 'ci.trigger.%'",
    )
    .fetch_all(pool.pool())
    .await
    .expect("query events")
    .into_iter()
    .map(|(_, payload)| {
        let payload: serde_json::Value = serde_json::from_str(&payload).expect("parse payload");
        (
            payload["ref_name"].as_str().unwrap_or_default().to_string(),
            payload["new_hash"].as_str().unwrap_or_default().to_string(),
        )
    })
    .collect()
}

/// A gated branch advance is declined with the synthesized receive-pack
/// rejection report, the ref does not advance, and a green pipeline run for
/// the commit lets the very same push through.
#[tokio::test]
async fn test_required_checks_reject_then_green_run_allows_push() {
    let mut server = spawn_server(&["ci"]).await;
    let (sha, work) = make_commit(&server, "gated");

    // The push is declined; the git client surfaces the policy reason.
    let rejected = try_git(&["push", "origin", "main"], &work);
    assert!(
        !rejected.status.success(),
        "push to a not-green commit must fail"
    );
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.contains("remote rejected") && stderr.contains("required checks not green"),
        "expected policy rejection diagnostics, got: {stderr}"
    );

    // Nothing advanced server-side: the branch does not exist in the bare
    // repository because the policy declined before receive-pack ran.
    let bare = bare_repo(&server);
    let probe = try_git(&["rev-parse", "refs/heads/main"], &bare);
    assert!(
        !probe.status.success(),
        "declined branch creation must not create the ref"
    );

    // The rejection happens before the CI outbox sees anything: a push that
    // never reached receive-pack must not mint a pipeline trigger.
    assert!(
        trigger_rows(&server).await.is_empty(),
        "a policy-rejected push must not enqueue a CI trigger"
    );

    // Once the commit's required check is green, the same push lands.
    seed_green_run(&server, &sha).await;
    run_git(&["push", "origin", "main"], &work, &[]);
    let advanced = run_git(&["rev-parse", "refs/heads/main"], &bare, &[]);
    assert_eq!(
        String::from_utf8_lossy(&advanced.stdout).trim(),
        sha,
        "green commit must advance the branch"
    );
    // The accepted push triggered CI exactly once.
    let rows = trigger_rows(&server).await;
    assert_eq!(
        rows,
        vec![("refs/heads/main".to_string(), sha)],
        "accepted push must enqueue exactly one CI trigger"
    );

    common::shutdown_gracefully(&mut server.child).await;
}

/// Tag pushes bypass the branch gate, and branch deletions are accepted
/// without minting a CI trigger.
#[tokio::test]
async fn test_tags_bypass_gate_and_deletions_trigger_nothing() {
    let mut server = spawn_server(&["ci"]).await;

    // A gated repo still accepts a tag pointing at a not-green commit.
    let (sha, work) = make_commit(&server, "tagged");
    run_git(&["tag", "v1"], &work, &[]);
    run_git(&["push", "origin", "v1"], &work, &[]);
    let tag = run_git(&["rev-parse", "refs/tags/v1"], &bare_repo(&server), &[]);
    assert_eq!(
        String::from_utf8_lossy(&tag.stdout).trim(),
        sha,
        "tag push must bypass the required-checks gate"
    );
    // Tag pushes still trigger CI — only deletions are filtered.
    assert_eq!(
        trigger_rows(&server).await,
        vec![("refs/tags/v1".to_string(), sha.clone())],
        "tag push must enqueue a CI trigger"
    );

    // Land the branch and a secondary branch so there is something to
    // delete, then delete the secondary one. The deletion must be accepted
    // (deletion of the bare repo's *current* branch is denied by git
    // plumbing on the platform, exactly as upstream git configures it) and
    // must not mint a trigger (its new hash is all-zero).
    seed_green_run(&server, &sha).await;
    run_git(&["push", "origin", "main"], &work, &[]);
    run_git(&["push", "origin", "main:refs/heads/topic"], &work, &[]);
    run_git(&["push", "origin", "--delete", "topic"], &work, &[]);
    let probe = try_git(&["rev-parse", "refs/heads/topic"], &bare_repo(&server));
    assert!(!probe.status.success(), "deleted branch must be gone");

    // Tag push, branch land, and the topic branch creation each minted a
    // trigger; the accepted deletion minted none (its new hash is zero and
    // the handler filters deletions before enqueueing).
    let rows = trigger_rows(&server).await;
    assert_eq!(
        rows,
        vec![
            ("refs/tags/v1".to_string(), sha.clone()),
            ("refs/heads/main".to_string(), sha.clone()),
            ("refs/heads/topic".to_string(), sha.clone()),
        ],
        "deletion must not enqueue a CI trigger: {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|(_, new_hash)| !new_hash.chars().all(|c| c == '0')),
        "no trigger may carry an all-zero new hash: {rows:?}"
    );

    common::shutdown_gracefully(&mut server.child).await;
}
