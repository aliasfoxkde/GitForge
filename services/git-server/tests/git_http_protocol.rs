//! End-to-end Git Smart HTTP protocol tests.
//!
//! These tests spawn the real `git-server` binary against a temporary
//! SQLite database and git root, then drive it with the actual `git`
//! client (clone/push/ls-remote). No HTTP layer is mocked: requests hit
//! the real axum router and the real `git upload-pack`/`git receive-pack`
//! child processes.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gitforge_common::{RepoId, UserId};

mod common;

/// Environment for a spawned git-server instance.
struct TestServer {
    child: tokio::process::Child,
    http_port: u16,
    git_root: std::path::PathBuf,
    repo_id: RepoId,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

use common::{free_port, run_git};

/// Spawn the real git-server binary with a prepared database containing a
/// single `testowner/proto` repository backed by a bare git repository.
async fn spawn_server() -> TestServer {
    spawn_server_with(&[]).await
}

/// Spawn the real git-server binary with additional environment variables
/// layered on top of the base configuration (e.g. CI trigger settings).
async fn spawn_server_with(extra_env: &[(&str, &str)]) -> TestServer {
    let unique = uuid::Uuid::new_v4();
    let base = std::env::temp_dir().join(format!("git-server-proto-{unique}"));
    let git_root = base.join("git");
    let db_path = base.join("gitforge.db");
    std::fs::create_dir_all(&git_root).expect("create git root");

    // Seed the database with owner and repository rows using the same
    // migration path the service uses at startup. A bare path (no scheme)
    // makes Pool::new create the file via ?mode=rwc.
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
    drop(pool);

    // The server resolves owner/repo to <GIT_ROOT>/<repo_id> (the
    // StorageBackend layout), so the bare repository must exist there.
    let bare_repo = git_root.join(repo_id.to_string());
    run_git(
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            bare_repo.to_str().unwrap(),
        ],
        &base,
        &[],
    );

    let http_port = free_port();
    let ssh_port = free_port();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_git-server"));
    command
        .env("HTTP_PORT", http_port.to_string())
        .env("SSH_PORT", ssh_port.to_string())
        .env("GIT_ROOT", &git_root)
        .env("DATABASE_URL", format!("sqlite:{}", db_path.display()))
        .env("RUST_LOG", "warn");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn git-server binary");

    // Wait for the HTTP listener to report healthy.
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

    TestServer {
        child,
        http_port,
        git_root,
        repo_id,
    }
}

#[tokio::test]
async fn test_git_push_and_clone_over_smart_http() {
    let mut server = spawn_server().await;
    let base = server.git_root.parent().unwrap().to_path_buf();
    let origin_url = format!("http://127.0.0.1:{}/testowner/proto.git", server.http_port);

    // ─── Push: real smart-HTTP receive-pack ─────────────────────────────
    let work = base.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    run_git(&["init", "--initial-branch=main"], &work, &[]);
    run_git(&["config", "user.email", "dev@example.com"], &work, &[]);
    run_git(&["config", "user.name", "Protocol Test"], &work, &[]);
    std::fs::write(work.join("hello.txt"), "git protocol smoke\n").expect("write file");
    run_git(&["add", "."], &work, &[]);
    run_git(&["commit", "-m", "protocol test commit"], &work, &[]);
    run_git(&["remote", "add", "origin", &origin_url], &work, &[]);
    run_git(&["push", "origin", "main"], &work, &[]);

    // The pushed commit must be durable in the bare repository on disk.
    let bare = server.git_root.join(server.repo_id.to_string());
    let pushed = run_git(&["rev-parse", "HEAD"], &bare, &[]);
    let pushed_sha = String::from_utf8_lossy(&pushed.stdout).trim().to_string();
    assert_eq!(pushed_sha.len(), 40, "expected full sha, got {pushed_sha}");
    let local = run_git(&["rev-parse", "HEAD"], &work, &[]);
    assert_eq!(
        String::from_utf8_lossy(&local.stdout).trim(),
        pushed_sha,
        "pushed commit must match local commit"
    );

    // ─── Clone: real smart-HTTP upload-pack ─────────────────────────────
    let clone_parent = base.join("clones");
    std::fs::create_dir_all(&clone_parent).expect("create clone parent");
    run_git(&["clone", &origin_url, "cloned"], &clone_parent, &[]);
    let cloned_file = std::fs::read_to_string(clone_parent.join("cloned").join("hello.txt"))
        .expect("cloned file");
    assert_eq!(cloned_file, "git protocol smoke\n");

    // ─── Fetch after a second push keeps the protocol path honest ───────
    std::fs::write(work.join("second.txt"), "second commit\n").expect("write second file");
    run_git(&["add", "."], &work, &[]);
    run_git(&["commit", "-m", "second protocol commit"], &work, &[]);
    run_git(&["push", "origin", "main"], &work, &[]);
    run_git(
        &["fetch", "origin"],
        clone_parent.join("cloned").as_path(),
        &[],
    );

    common::shutdown_gracefully(&mut server.child).await;
}

#[tokio::test]
async fn test_ls_remote_unknown_repository_fails() {
    let mut server = spawn_server().await;
    let base = server.git_root.parent().unwrap().to_path_buf();
    let missing_url = format!(
        "http://127.0.0.1:{}/testowner/does-not-exist.git",
        server.http_port
    );

    std::fs::create_dir_all(&base).expect("create base");
    let output = Command::new("git")
        .args(["ls-remote", &missing_url])
        .current_dir(&base)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("spawn git ls-remote");
    assert!(
        !output.status.success(),
        "ls-remote of unknown repository must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found") || stderr.contains("404"),
        "expected repository-not-found diagnostics, got: {stderr}"
    );

    common::shutdown_gracefully(&mut server.child).await;
}

/// A local CI trigger endpoint that accepts TCP connections and never
/// responds for the lifetime of the test.
///
/// This is the only injected component in the issue-#288 regression: a
/// real, bounded, local listener with the exact failure shape observed
/// live (CI accepted the connection but produced no HTTP response). The
/// sockets are held open without reading or writing — a trigger payload is
/// far smaller than kernel socket buffers, so the client's POST lands and
/// the client then simply waits for a response that never comes. The
/// returned counter records every accepted connection so the test can
/// prove delivery attempts happen without the push response depending on
/// them.
async fn hung_ci_receiver() -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local hung CI receiver");
    let port = listener.local_addr().expect("local addr").port();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_task = hits.clone();
    tokio::spawn(async move {
        // Held sockets keep the clients waiting; dropping them early would
        // turn the hang into a connection error instead.
        let mut held = Vec::new();
        while let Ok((socket, _addr)) = listener.accept().await {
            hits_task.fetch_add(1, Ordering::SeqCst);
            held.push(socket);
        }
    });
    (port, hits)
}

/// Regression for issue #288: an already-accepted push must not wait on
/// downstream CI trigger delivery.
///
/// `git_receive_pack` used to await `deliver_pending_ci_events` inline
/// before writing its response. That function performs real network sends
/// (60 s HTTP timeout per row, up to 50 rows sequentially), so a slow or
/// hung CI endpoint pinned the whole push — live, a real push timed out at
/// 150 s and the branch ref never reached the client even though git had
/// accepted the pack. The durable `ci_delivery_loop` (2 s cadence,
/// lease-claimed, retried) already exists for exactly this delivery, so
/// the receive-pack response must return as soon as the pack is accepted
/// and the trigger row is durable.
///
/// Red/green contract: against the hung receiver the push must complete
/// within a bound far below the 60 s HTTP-client timeout, the ref must
/// land, and the background worker must attempt delivery asynchronously.
#[tokio::test]
async fn test_push_response_not_blocked_by_hung_ci_trigger() {
    // ─── Local hung CI trigger endpoint (the only test dependency) ──────
    let (ci_port, ci_hits) = hung_ci_receiver().await;

    let mut server = spawn_server_with(&[
        (
            "GITFORGE_CI_TRIGGER_URL",
            &format!("http://127.0.0.1:{ci_port}/internal/ci/trigger"),
        ),
        ("GITFORGE_CI_TRIGGER_TOKEN", "test-ci-trigger-token"),
    ])
    .await;
    let base = server.git_root.parent().unwrap().to_path_buf();
    let origin_url = format!("http://127.0.0.1:{}/testowner/proto.git", server.http_port);

    let work = base.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    run_git(&["init", "--initial-branch=main"], &work, &[]);
    run_git(&["config", "user.email", "dev@example.com"], &work, &[]);
    run_git(&["config", "user.name", "Outbox Regression"], &work, &[]);
    std::fs::write(work.join("outbox.txt"), "push must not wait on ci\n").expect("write file");
    run_git(&["add", "."], &work, &[]);
    run_git(&["commit", "-m", "outbox regression commit"], &work, &[]);
    run_git(&["remote", "add", "origin", &origin_url], &work, &[]);

    // ─── Push under a hard bound far below the 60 s delivery timeout ────
    // The bound must stay below `ci_http_client`'s 60 s request timeout so
    // a red run fails fast instead of merely looking slow; the normal push
    // here takes about a second.
    const PUSH_BOUND: Duration = Duration::from_secs(25);
    let mut push = tokio::process::Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(&work)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn git push");
    let started = tokio::time::Instant::now();
    let waited = tokio::time::timeout(PUSH_BOUND, push.wait()).await;
    let push_elapsed = started.elapsed();
    match waited {
        Ok(Ok(status)) => {
            assert!(
                status.success(),
                "git push must succeed once the response is unblocked (status: {status})"
            );
        }
        Ok(Err(error)) => panic!("git push wait failed: {error}"),
        Err(_) => {
            let _ = push.start_kill();
            // Reap the killed child so it does not linger as a zombie with
            // piped stdio for the rest of the test process.
            let _ = push.wait().await;
            panic!(
                "git push did not return within {PUSH_BOUND:?} (took {push_elapsed:?}); \
                 the accepted push is blocked on downstream CI trigger delivery (issue #288)"
            );
        }
    }

    // ─── The accepted ref must have landed ──────────────────────────────
    let bare = server.git_root.join(server.repo_id.to_string());
    let pushed = run_git(&["rev-parse", "refs/heads/main"], &bare, &[]);
    let local = run_git(&["rev-parse", "refs/heads/main"], &work, &[]);
    assert_eq!(
        String::from_utf8_lossy(&local.stdout).trim(),
        String::from_utf8_lossy(&pushed.stdout).trim(),
        "the pushed ref must be durable on the server"
    );

    // ─── Delivery is owned by the durable background worker ────────────
    // The hung endpoint must see at least one real delivery attempt after
    // the response returned, proving the trigger row was durably persisted
    // and is being retried asynchronously (2 s loop cadence) rather than
    // dropped with the inline path.
    let mut observed = false;
    for _ in 0..30 {
        if ci_hits.load(Ordering::SeqCst) >= 1 {
            observed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        observed,
        "the durable outbox must attempt CI delivery in the background after the push returned"
    );

    // ─── Exactly one durable trigger row, still awaiting delivery ───────
    // The row must exist (persistence preserved), be unique (no duplicate
    // enqueues), and still be pending/delivering (the hung endpoint means
    // it can never be marked delivered; the lease/retry machinery keeps
    // owning it).
    let reader = gitforge_db::Pool::new(&base.join("gitforge.db").display().to_string())
        .await
        .expect("open read-only pool over the server database");
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT event_type FROM events WHERE event_type LIKE 'ci.trigger.%'")
            .fetch_all(reader.pool())
            .await
            .expect("query trigger rows");
    assert_eq!(
        rows.len(),
        1,
        "exactly one durable trigger row; got {rows:?}"
    );
    assert!(
        rows[0].0 == "ci.trigger.pending" || rows[0].0 == "ci.trigger.delivering",
        "the trigger must still be awaiting/retrying delivery against the hung endpoint, got {}",
        rows[0].0
    );

    common::shutdown_gracefully(&mut server.child).await;
}
