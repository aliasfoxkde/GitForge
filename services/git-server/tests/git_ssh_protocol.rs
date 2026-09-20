//! End-to-end Git over SSH protocol tests.
//!
//! These tests spawn the real `git-server` binary (which serves SSH via
//! russh) against a temporary SQLite database and git root, generate a real
//! ed25519 client keypair with `ssh-keygen`, and drive the server with the
//! actual `git` client over the `ssh://` transport: ls-remote, push, clone,
//! and fetch. Nothing is mocked — the SSH handshake, public-key
//! authentication, channel multiplexing, and the git child processes are
//! all exercised for real.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use gitforge_common::{RepoId, UserId};

mod common;

/// Environment for a spawned git-server instance.
struct TestServer {
    child: tokio::process::Child,
    ssh_port: u16,
    git_root: PathBuf,
    repo_id: RepoId,
    /// `GIT_SSH_COMMAND` that authenticates with the generated client key.
    git_ssh_command: String,
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

fn run_git(
    args: &[&str],
    cwd: &Path,
    envs: &[(&str, &str)],
    git_ssh_command: Option<&str>,
) -> std::process::Output {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null());
    if let Some(ssh) = git_ssh_command {
        command.env("GIT_SSH_COMMAND", ssh);
    }
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

/// An OpenSSH option list shared by every ssh invocation in these tests:
/// trust only the pinned host key, present only the generated client key,
/// and never fall back to interactive prompts.
fn ssh_options(client_key: &Path, known_hosts: &Path) -> String {
    format!(
        "ssh -i {} -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes \
         -o UserKnownHostsFile={} -o BatchMode=yes -o LogLevel=ERROR",
        client_key.display(),
        known_hosts.display()
    )
}

/// Write a `known_hosts` file pinning the server's generated host key for
/// 127.0.0.1:<port>, exactly as a real client would pin it.
fn pin_host_key(base: &Path, port: u16, host_public_key: &str) -> PathBuf {
    let known_hosts = base.join("known_hosts");
    std::fs::write(
        &known_hosts,
        format!("[127.0.0.1]:{port} {host_public_key}\n"),
    )
    .expect("write known_hosts");
    known_hosts
}

/// Spawn the real git-server binary with a prepared database containing a
/// single `testowner/proto` repository backed by a bare git repository, a
/// generated host key, and a generated client keypair.
async fn spawn_server() -> TestServer {
    let unique = uuid::Uuid::new_v4();
    let base = std::env::temp_dir().join(format!("git-server-ssh-{unique}"));
    let git_root = base.join("git");
    let ssh_dir = base.join("ssh");
    let db_path = base.join("gitforge.db");
    std::fs::create_dir_all(&git_root).expect("create git root");
    std::fs::create_dir_all(&ssh_dir).expect("create ssh dir");

    // Seed the database with owner and repository rows using the same
    // migration path the service uses at startup.
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

    // Generate the ed25519 client keypair the tests will authenticate with.
    let client_key = ssh_dir.join("client_ed25519");
    let keygen = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            client_key.to_str().unwrap(),
            "-C",
            "gitforge-ssh-test",
        ])
        .output()
        .expect("spawn ssh-keygen");
    assert!(
        keygen.status.success(),
        "ssh-keygen failed: {}",
        String::from_utf8_lossy(&keygen.stderr)
    );

    // Register the generated client key to the owner account before the
    // server starts (as a user would through the API), because the
    // transport rejects unregistered fingerprints.
    let client_public =
        std::fs::read_to_string(client_key.with_extension("pub")).expect("read client public key");
    let parsed_client_key = russh::keys::ssh_key::PublicKey::from_openssh(&client_public)
        .expect("parse client public key");
    let key_record = gitforge_db::models::SshKey::new(
        user_id,
        "test-client-key".to_string(),
        parsed_client_key
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string(),
        client_public.trim().to_string(),
    );
    gitforge_db::queries::SshKeyQueries::create(&pool, &key_record)
        .await
        .expect("register client ssh key");

    drop(pool);

    // The server resolves owner/repo to <GIT_ROOT>/<repo_id>, so the bare
    // repository must exist there.
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
        None,
    );

    let host_key_path = ssh_dir.join("host_ed25519");
    let ssh_port = free_port();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_git-server"))
        .env("HTTP_PORT", free_port().to_string())
        .env("SSH_PORT", ssh_port.to_string())
        .env("GIT_ROOT", &git_root)
        .env("DATABASE_URL", format!("sqlite:{}", db_path.display()))
        .env("GITFORGE_SSH_HOST_KEY", &host_key_path)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn git-server binary");

    // The server reports readiness via its HTTP health endpoint; the SSH
    // listener starts alongside it. The host key exists only after the
    // server wrote it, so wait for that file before pinning it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if tokio::time::Instant::now() >= deadline {
            let _ = child.start_kill();
            panic!("git-server did not write its host key at {host_key_path:?}");
        }
        if host_key_path.with_extension("pub").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Pin the host key the server just published, then confirm the SSH
    // transport accepts the pinned key before handing over to the tests.
    let host_public =
        std::fs::read_to_string(host_key_path.with_extension("pub")).expect("read host public key");
    let known_hosts = pin_host_key(&base, ssh_port, host_public.trim());

    let git_ssh_command = ssh_options(&client_key, &known_hosts);

    TestServer {
        child,
        ssh_port,
        git_root,
        repo_id,
        git_ssh_command,
    }
}

fn ssh_url(port: u16, path: &str) -> String {
    format!("ssh://gitforge@127.0.0.1:{port}/{path}")
}

#[tokio::test]
async fn test_git_push_and_clone_over_ssh() {
    let mut server = spawn_server().await;
    let base = server.git_root.parent().unwrap().to_path_buf();
    let origin_url = ssh_url(server.ssh_port, "testowner/proto.git");
    let ssh = Some(server.git_ssh_command.as_str());

    // ─── Push: real receive-pack over an authenticated SSH channel ──────
    let work = base.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    run_git(&["init", "--initial-branch=main"], &work, &[], None);
    run_git(
        &["config", "user.email", "dev@example.com"],
        &work,
        &[],
        None,
    );
    run_git(
        &["config", "user.name", "SSH Protocol Test"],
        &work,
        &[],
        None,
    );
    std::fs::write(work.join("hello.txt"), "git over ssh smoke\n").expect("write file");
    run_git(&["add", "."], &work, &[], None);
    run_git(
        &["commit", "-m", "ssh protocol test commit"],
        &work,
        &[],
        None,
    );
    run_git(&["remote", "add", "origin", &origin_url], &work, &[], None);
    run_git(&["push", "origin", "main"], &work, &[], ssh);

    // The pushed commit must be durable in the bare repository on disk,
    // and the pushed ref must actually move (the old advertisement-only
    // handler never updated refs).
    let bare = server.git_root.join(server.repo_id.to_string());
    let pushed = run_git(&["rev-parse", "refs/heads/main"], &bare, &[], None);
    let pushed_sha = String::from_utf8_lossy(&pushed.stdout).trim().to_string();
    assert_eq!(pushed_sha.len(), 40, "expected full sha, got {pushed_sha}");
    let local = run_git(&["rev-parse", "HEAD"], &work, &[], None);
    assert_eq!(
        String::from_utf8_lossy(&local.stdout).trim(),
        pushed_sha,
        "pushed ref must match local commit"
    );

    // ─── Clone: real upload-pack negotiation over SSH ───────────────────
    let clone_parent = base.join("clones");
    std::fs::create_dir_all(&clone_parent).expect("create clone parent");
    run_git(&["clone", &origin_url, "cloned"], &clone_parent, &[], ssh);
    let cloned_file = std::fs::read_to_string(clone_parent.join("cloned").join("hello.txt"))
        .expect("cloned file");
    assert_eq!(cloned_file, "git over ssh smoke\n");

    // ─── Fetch after a second push keeps the transport honest ───────────
    std::fs::write(work.join("second.txt"), "second commit\n").expect("write file");
    run_git(&["add", "."], &work, &[], None);
    run_git(
        &["commit", "-m", "second ssh protocol commit"],
        &work,
        &[],
        None,
    );
    run_git(&["push", "origin", "main"], &work, &[], ssh);
    run_git(&["fetch", "origin"], &clone_parent.join("cloned"), &[], ssh);

    common::shutdown_gracefully(&mut server.child).await;
}

#[tokio::test]
async fn test_ls_remote_over_ssh_lists_pushed_refs() {
    let mut server = spawn_server().await;
    let base = server.git_root.parent().unwrap().to_path_buf();
    let origin_url = ssh_url(server.ssh_port, "testowner/proto.git");
    let ssh = Some(server.git_ssh_command.as_str());

    // Seed one commit so ls-remote has a ref to report.
    let work = base.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    run_git(&["init", "--initial-branch=main"], &work, &[], None);
    run_git(
        &["config", "user.email", "dev@example.com"],
        &work,
        &[],
        None,
    );
    run_git(
        &["config", "user.name", "SSH Protocol Test"],
        &work,
        &[],
        None,
    );
    std::fs::write(work.join("seed.txt"), "seed\n").expect("write file");
    run_git(&["add", "."], &work, &[], None);
    run_git(&["commit", "-m", "seed commit"], &work, &[], None);
    run_git(&["remote", "add", "origin", &origin_url], &work, &[], None);
    run_git(&["push", "origin", "main"], &work, &[], ssh);

    let listed = run_git(&["ls-remote", &origin_url], &base, &[], ssh);
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        stdout.contains("refs/heads/main"),
        "ls-remote must list pushed refs, got: {stdout}"
    );

    common::shutdown_gracefully(&mut server.child).await;
}

#[tokio::test]
async fn test_ssh_unregistered_key_is_rejected() {
    let mut server = spawn_server().await;
    let base = server.git_root.parent().unwrap().to_path_buf();
    let origin_url = ssh_url(server.ssh_port, "testowner/proto.git");
    let known_hosts = base.join("known_hosts");

    // A second, real keypair that was never registered to any account.
    // The transport must not authenticate it on possession alone.
    let stranger_key = base.join("stranger_ed25519");
    let keygen = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            stranger_key.to_str().unwrap(),
            "-C",
            "unregistered-stranger",
        ])
        .output()
        .expect("spawn ssh-keygen");
    assert!(
        keygen.status.success(),
        "ssh-keygen failed: {}",
        String::from_utf8_lossy(&keygen.stderr)
    );

    let stranger_command = ssh_options(&stranger_key, &known_hosts);
    let output = Command::new("git")
        .args(["ls-remote", &origin_url])
        .current_dir(&base)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", &stranger_command)
        .output()
        .expect("spawn git ls-remote");
    assert!(
        !output.status.success(),
        "an unregistered key must not authenticate"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Permission denied"),
        "expected public-key denial, got: {stderr}"
    );

    common::shutdown_gracefully(&mut server.child).await;
}

#[tokio::test]
async fn test_ssh_without_client_key_is_rejected() {
    let mut server = spawn_server().await;
    let base = server.git_root.parent().unwrap().to_path_buf();
    let origin_url = ssh_url(server.ssh_port, "testowner/proto.git");

    // The server requires a public key: a client that presents none must
    // not get a session, let alone repository access.
    let known_hosts = base.join("known_hosts");
    let no_key_command = format!(
        "ssh -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes \
         -o UserKnownHostsFile={} -o BatchMode=yes -o LogLevel=ERROR \
         -o PubkeyAuthentication=no",
        known_hosts.display()
    );
    let output = Command::new("git")
        .args(["ls-remote", &origin_url])
        .current_dir(&base)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", &no_key_command)
        .output()
        .expect("spawn git ls-remote");
    assert!(
        !output.status.success(),
        "SSH without a client key must fail, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    common::shutdown_gracefully(&mut server.child).await;
}

#[tokio::test]
async fn test_ssh_unknown_repository_fails() {
    let mut server = spawn_server().await;
    let base = server.git_root.parent().unwrap().to_path_buf();
    let missing_url = ssh_url(server.ssh_port, "testowner/does-not-exist.git");

    let output = Command::new("git")
        .args(["ls-remote", &missing_url])
        .current_dir(&base)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", server.git_ssh_command.clone())
        .output()
        .expect("spawn git ls-remote");
    assert!(
        !output.status.success(),
        "ls-remote of unknown repository must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "expected repository-not-found diagnostics, got: {stderr}"
    );

    common::shutdown_gracefully(&mut server.child).await;
}
