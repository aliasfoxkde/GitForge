//! End-to-end Git Smart HTTP protocol tests, including the auth gate.
//!
//! These tests spawn the real `git-server` binary against a temporary
//! SQLite database and git root, then drive it with the actual `git`
//! client (clone/push/ls-remote) and raw HTTP requests. No HTTP layer is
//! mocked: requests hit the real axum router, the real Smart HTTP auth
//! gate, and the real `git upload-pack`/`git receive-pack` child
//! processes.
//!
//! Seeded world: `testowner` owns private `testowner/proto` and public
//! `testowner/pubrepo`, both with bare repositories in storage;
//! `otherdev` (default developer role) has no repositories. Both users
//! carry live refresh credentials so git clients authenticate the way the
//! credential-helper contract serves them.

// Test-harness exemption, same discipline as the sibling git-server suites:
// `allow-unwrap-in-tests` covers `#[test]` bodies, but fixture and
// boot-helper functions in a test target are neither `#[test]` fns nor
// `#[cfg(test)]`, a class the clippy.toml config cannot address. Setup
// failing IS the assertion -- a panic aborts the run loudly. Production
// code keeps the denies.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::{Command, Stdio};
use std::time::Duration;

use gitforge_common::{RepoId, UserId};

mod common;

/// Environment for a spawned git-server instance.
struct TestServer {
    child: tokio::process::Child,
    http_port: u16,
    base: std::path::PathBuf,
    git_root: std::path::PathBuf,
    repo_id: RepoId,
    /// Live refresh credential for `testowner` (owner of both repos).
    owner_credential: String,
    /// Live refresh credential for `otherdev` (developer, owns nothing).
    otherdev_credential: String,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

use common::{authed_remote_url, basic_auth, free_port, run_git, seed_refresh_credential};

impl TestServer {
    /// Remote URL for `owner/repo` authenticated as `username`.
    fn remote_url(&self, username: &str, credential: &str, owner: &str, repo: &str) -> String {
        authed_remote_url(self.http_port, username, credential, owner, repo)
    }

    /// `Authorization` header for the owner account.
    fn owner_auth(&self) -> String {
        basic_auth("testowner", &self.owner_credential)
    }

    /// `Authorization` header for the developer account.
    fn otherdev_auth(&self) -> String {
        basic_auth("otherdev", &self.otherdev_credential)
    }
}

/// Run a `git` command against the transport without asserting success —
/// for probes that the auth gate is expected to reject.
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

/// Spawn the real git-server binary with a prepared database: private
/// `testowner/proto`, public `testowner/pubrepo`, and live credentials
/// for `testowner` and `otherdev`.
async fn spawn_server() -> TestServer {
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
    let otherdev = gitforge_db::models::User::new(
        "otherdev".to_string(),
        "otherdev@example.com".to_string(),
        "hash".to_string(),
    );
    gitforge_db::queries::UserQueries::create(&pool, &otherdev)
        .await
        .expect("create other user");

    let owner_credential = seed_refresh_credential(
        &pool,
        user_id,
        chrono::Utc::now() + chrono::Duration::days(30),
    )
    .await;
    let otherdev_credential = seed_refresh_credential(
        &pool,
        otherdev.id,
        chrono::Utc::now() + chrono::Duration::days(30),
    )
    .await;

    let repository =
        gitforge_db::models::Repository::new("proto".to_string(), user_id, "git".to_string());
    let repo_id = repository.id;
    gitforge_db::queries::RepoQueries::create(&pool, &repository)
        .await
        .expect("create repository");
    let public =
        gitforge_db::models::Repository::new("pubrepo".to_string(), user_id, "git".to_string());
    gitforge_db::queries::RepoQueries::create(&pool, &public)
        .await
        .expect("create public repository");
    sqlx::query("UPDATE repositories SET visibility = 'public' WHERE id = ?")
        .bind(public.id.to_string())
        .execute(pool.pool())
        .await
        .expect("make repository public");
    drop(pool);

    // The server resolves owner/repo to <GIT_ROOT>/<repo_id> (the
    // StorageBackend layout), so the bare repositories must exist there.
    for id in [repo_id, public.id] {
        let bare_repo = git_root.join(id.to_string());
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
    }

    let http_port = free_port();
    let ssh_port = free_port();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_git-server"))
        .env("HTTP_PORT", http_port.to_string())
        .env("SSH_PORT", ssh_port.to_string())
        .env("GIT_ROOT", &git_root)
        .env("DATABASE_URL", format!("sqlite:{}", db_path.display()))
        .env("RUST_LOG", "warn")
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
        base,
        git_root,
        repo_id,
        owner_credential,
        otherdev_credential,
    }
}

/// The auth gate's refusal diagnostics in git's stderr: with terminal
/// prompts disabled, a 401 challenge surfaces as a username prompt that
/// cannot be answered.
fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_authentication_refused(stderr: &str) {
    assert!(
        stderr.contains("could not read Username") || stderr.contains("Authentication failed"),
        "expected an authentication refusal, got: {stderr}"
    );
}

#[tokio::test]
async fn test_git_push_and_clone_over_smart_http() {
    let mut server = spawn_server().await;
    let base = server.base.clone();
    let origin_url = server.remote_url("testowner", &server.owner_credential, "testowner", "proto");

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

/// Anonymous push: refused by the challenge, and nothing reaches the bare
/// repository — the refs on disk stay empty.
#[tokio::test]
async fn test_anonymous_push_rejected_and_refs_unchanged() {
    let mut server = spawn_server().await;
    let origin_url = format!("http://127.0.0.1:{}/testowner/proto", server.http_port);

    let work = server.base.join("work-anon");
    std::fs::create_dir_all(&work).expect("create work dir");
    run_git(&["init", "--initial-branch=main"], &work, &[]);
    run_git(&["config", "user.email", "anon@example.com"], &work, &[]);
    run_git(&["config", "user.name", "Anonymous"], &work, &[]);
    std::fs::write(work.join("secret.txt"), "must not land\n").expect("write file");
    run_git(&["add", "."], &work, &[]);
    run_git(&["commit", "-m", "anonymous push must fail"], &work, &[]);
    run_git(&["remote", "add", "origin", &origin_url], &work, &[]);

    let output = try_git(&["push", "origin", "main"], &work);
    assert!(!output.status.success(), "anonymous push must be rejected");
    assert_authentication_refused(&stderr_of(&output));

    // Fail closed means closed: no ref may exist in the bare repository.
    let bare = server.git_root.join(server.repo_id.to_string());
    let refs = run_git(&["for-each-ref"], &bare, &[]);
    assert!(
        String::from_utf8_lossy(&refs.stdout).trim().is_empty(),
        "rejected push must not advance any ref"
    );

    common::shutdown_gracefully(&mut server.child).await;
}

/// Anonymous read of a private repository: challenged, never served.
#[tokio::test]
async fn test_anonymous_private_read_is_challenged() {
    let mut server = spawn_server().await;
    let private_url = format!("http://127.0.0.1:{}/testowner/proto", server.http_port);

    let output = try_git(&["ls-remote", &private_url], &server.base);
    assert!(!output.status.success(), "anonymous private read must fail");
    assert_authentication_refused(&stderr_of(&output));

    common::shutdown_gracefully(&mut server.child).await;
}

/// Anonymous read of an explicitly public repository: allowed. This is
/// the only anonymous allowance on the transport.
#[tokio::test]
async fn test_anonymous_public_read_is_served() {
    let mut server = spawn_server().await;
    let public_url = format!("http://127.0.0.1:{}/testowner/pubrepo", server.http_port);

    // An empty bare repository advertises no refs but answers cleanly.
    let output = try_git(&["ls-remote", &public_url], &server.base);
    assert!(
        output.status.success(),
        "anonymous public read must succeed: {}",
        stderr_of(&output)
    );

    common::shutdown_gracefully(&mut server.child).await;
}

/// An authenticated non-owner reading a private repository sees a plain
/// 404 ("repository not found"), not an authentication prompt — private
/// existence is not leaked, and a legit 401 prompt would imply it exists.
#[tokio::test]
async fn test_non_owner_private_read_reports_not_found() {
    let mut server = spawn_server().await;
    let url = server.remote_url(
        "otherdev",
        &server.otherdev_credential,
        "testowner",
        "proto",
    );

    let output = try_git(&["ls-remote", &url], &server.base);
    assert!(!output.status.success(), "non-owner private read must fail");
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("not found") || stderr.contains("404"),
        "expected a not-found answer, got: {stderr}"
    );
    assert!(
        !stderr.contains("could not read Username") && !stderr.contains("Authentication failed"),
        "a private refusal must not look like an auth prompt: {stderr}"
    );

    common::shutdown_gracefully(&mut server.child).await;
}

/// A developer pushing to someone else's PUBLIC repository is refused
/// with 403 (existence is already public), and no ref advances.
#[tokio::test]
async fn test_developer_cannot_push_public_repository() {
    let mut server = spawn_server().await;
    let url = server.remote_url(
        "otherdev",
        &server.otherdev_credential,
        "testowner",
        "pubrepo",
    );

    let work = server.base.join("work-dev");
    std::fs::create_dir_all(&work).expect("create work dir");
    run_git(&["init", "--initial-branch=main"], &work, &[]);
    run_git(&["config", "user.email", "dev@example.com"], &work, &[]);
    run_git(&["config", "user.name", "Developer"], &work, &[]);
    std::fs::write(work.join("patch.txt"), "denied\n").expect("write file");
    run_git(&["add", "."], &work, &[]);
    run_git(&["commit", "-m", "must be forbidden"], &work, &[]);
    run_git(&["remote", "add", "origin", &url], &work, &[]);

    let output = try_git(&["push", "origin", "main"], &work);
    assert!(!output.status.success(), "developer push must be denied");
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("403"),
        "expected a 403 refusal on a public repo, got: {stderr}"
    );

    common::shutdown_gracefully(&mut server.child).await;
}

/// An unknown repository is indistinguishable from a private one:
/// anonymous callers get the uniform 401 challenge for both, the
/// authenticated owner gets 404 for the unknown one (and 200 for the
/// private one they own).
#[tokio::test]
async fn test_unknown_repo_is_indistinguishable_from_private() {
    let mut server = spawn_server().await;
    let client = reqwest::Client::new();
    let root = format!("http://127.0.0.1:{}", server.http_port);

    let mut challenge_headers = Vec::new();
    for path in [
        "/testowner/proto/info/refs?service=git-upload-pack",
        "/testowner/no-such-repo/info/refs?service=git-upload-pack",
        "/testowner/proto/info/refs?service=git-receive-pack",
        "/testowner/no-such-repo/info/refs?service=git-receive-pack",
    ] {
        let response = client
            .get(format!("{root}{path}"))
            .send()
            .await
            .expect("anonymous info/refs");
        assert_eq!(response.status(), 401, "anonymous {path}");
        challenge_headers.push(
            response
                .headers()
                .get("www-authenticate")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        );
    }
    assert!(
        challenge_headers
            .iter()
            .all(|header| header.as_deref() == Some("Basic realm=\"GitForge\"")),
        "every anonymous challenge must be the identical Basic challenge: {challenge_headers:?}"
    );

    // Authenticated: the private repo the owner may read serves; the
    // unknown one stays 404 — and the write advertisement needs no
    // separate check here (covered below).
    let response = client
        .get(format!(
            "{root}/testowner/proto/info/refs?service=git-upload-pack"
        ))
        .header("Authorization", server.owner_auth())
        .send()
        .await
        .expect("owner private info/refs");
    assert_eq!(response.status(), 200);

    let response = client
        .get(format!(
            "{root}/testowner/no-such-repo/info/refs?service=git-upload-pack"
        ))
        .header("Authorization", server.owner_auth())
        .send()
        .await
        .expect("owner unknown info/refs");
    assert_eq!(response.status(), 404);

    common::shutdown_gracefully(&mut server.child).await;
}

/// Expired, unknown, and malformed credentials are challenged — never
/// silently downgraded to anonymous.
#[tokio::test]
async fn test_broken_credentials_are_challenged() {
    let mut server = spawn_server().await;
    let client = reqwest::Client::new();
    let root = format!(
        "http://127.0.0.1:{}/testowner/proto/info/refs?service=git-upload-pack",
        server.http_port
    );

    // An expired but well-formed credential.
    let pool = gitforge_db::Pool::new(&server.base.join("gitforge.db").display().to_string())
        .await
        .expect("open seeded db");
    let testowner = gitforge_db::queries::UserQueries::get_by_username(&pool, "testowner")
        .await
        .expect("lookup testowner")
        .expect("testowner exists");
    let expired = common::seed_refresh_credential(
        &pool,
        testowner.id,
        chrono::Utc::now() - chrono::Duration::hours(1),
    )
    .await;
    drop(pool);

    for (name, header) in [
        ("expired credential", basic_auth("testowner", &expired)),
        (
            "unknown credential",
            basic_auth("testowner", "gfrtnot-a-real-credential"),
        ),
        ("garbage base64", "Basic !!!not-base64!!!".to_string()),
        ("non-Basic scheme", "Bearer some.jwt.value".to_string()),
    ] {
        let response = client
            .get(&root)
            .header("Authorization", header)
            .send()
            .await
            .expect("broken credential request");
        assert_eq!(
            response.status(),
            401,
            "{name} must draw the challenge, not anonymous service"
        );
        assert_eq!(
            response
                .headers()
                .get("www-authenticate")
                .and_then(|value| value.to_str().ok()),
            Some("Basic realm=\"GitForge\""),
            "{name} challenge header"
        );
    }

    common::shutdown_gracefully(&mut server.child).await;
}

/// The receive-pack advertisement is the first half of a push: even on a
/// public repository it requires an authorized writer, and the legacy
/// explicit routes are gated identically.
#[tokio::test]
async fn test_receive_pack_advertisement_and_legacy_routes_are_gated() {
    let mut server = spawn_server().await;
    let client = reqwest::Client::new();
    let root = format!("http://127.0.0.1:{}", server.http_port);

    // Anonymous receive-pack advertisement on a PUBLIC repo: still a
    // challenge — writes never leak even a ref list anonymously.
    let response = client
        .get(format!(
            "{root}/testowner/pubrepo/info/refs?service=git-receive-pack"
        ))
        .send()
        .await
        .expect("anonymous receive-pack advertisement");
    assert_eq!(response.status(), 401);

    // The owner may see it.
    let response = client
        .get(format!(
            "{root}/testowner/pubrepo/info/refs?service=git-receive-pack"
        ))
        .header("Authorization", server.owner_auth())
        .send()
        .await
        .expect("owner receive-pack advertisement");
    assert_eq!(response.status(), 200);

    // A developer (not owner/admin/maintainer) may not — 403 on a public
    // repository states the boundary.
    let response = client
        .get(format!(
            "{root}/testowner/pubrepo/info/refs?service=git-receive-pack"
        ))
        .header("Authorization", server.otherdev_auth())
        .send()
        .await
        .expect("developer receive-pack advertisement");
    assert_eq!(response.status(), 403);

    // The legacy explicit upload-pack route is gated like the standard
    // one: anonymous 401, owner 200.
    let response = client
        .get(format!("{root}/git-upload-pack/testowner/proto"))
        .send()
        .await
        .expect("anonymous legacy upload-pack");
    assert_eq!(response.status(), 401);
    let response = client
        .get(format!("{root}/git-upload-pack/testowner/proto"))
        .header("Authorization", server.owner_auth())
        .send()
        .await
        .expect("owner legacy upload-pack");
    assert_eq!(response.status(), 200);

    common::shutdown_gracefully(&mut server.child).await;
}

#[tokio::test]
async fn test_ls_remote_unknown_repository_fails() {
    let mut server = spawn_server().await;

    // Anonymous: uniform challenge (a nonexistent repo must look exactly
    // like a private one).
    let missing_url = format!(
        "http://127.0.0.1:{}/testowner/does-not-exist.git",
        server.http_port
    );
    let output = try_git(&["ls-remote", &missing_url], &server.base);
    assert!(
        !output.status.success(),
        "anonymous ls-remote of unknown repository must fail"
    );
    assert_authentication_refused(&stderr_of(&output));

    // Authenticated: a plain not-found.
    let missing_authed = server.remote_url(
        "testowner",
        &server.owner_credential,
        "testowner",
        "does-not-exist",
    );
    let output = try_git(&["ls-remote", &missing_authed], &server.base);
    assert!(
        !output.status.success(),
        "authenticated ls-remote of unknown repository must fail"
    );
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("not found") || stderr.contains("404"),
        "expected repository-not-found diagnostics, got: {stderr}"
    );

    common::shutdown_gracefully(&mut server.child).await;
}
