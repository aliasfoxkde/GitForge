//! End-to-end Git Smart HTTP protocol tests.
//!
//! These tests spawn the real `git-server` binary against a temporary
//! SQLite database and git root, then drive it with the actual `git`
//! client (clone/push/ls-remote). No HTTP layer is mocked: requests hit
//! the real axum router and the real `git upload-pack`/`git receive-pack`
//! child processes.

use std::process::{Command, Stdio};
use std::time::Duration;

use gitforge_common::{RepoId, UserId};

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

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn run_git(args: &[&str], cwd: &std::path::Path, envs: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null());
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("spawn git");
    if !output.status.success() {
        panic!(
            "git {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    output
}

/// Spawn the real git-server binary with a prepared database containing a
/// single `testowner/proto` repository backed by a bare git repository.
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
        &clone_parent.join("cloned").as_path(),
        &[],
    );

    let _ = server.child.start_kill();
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

    let _ = server.child.start_kill();
}
