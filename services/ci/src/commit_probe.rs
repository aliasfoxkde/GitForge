//! Probe whether a commit can be checked out from a repository's storage.
//!
//! A CI run is only meaningful for a commit its workspace can actually
//! check out. The workspace is a clone of the repository's bare storage, so
//! a commit the storage does not hold — or holds only under a ref the
//! clone does not fetch — yields a run that can only fail, with
//! `fatal: unable to read tree <sha>` as its reason.
//!
//! Two trigger paths race for every push: an external relay that learns
//! about a push before the mirror has received the objects, and the
//! mirror's own receive path, which arrives later. Accepting the first
//! without a presence check creates a doomed run for every push and a
//! second, real run behind it.

use std::path::Path;
use std::time::Duration;

/// The state of a commit in a repository's storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitAvailability {
    /// The commit exists and a ref the workspace clone fetches reaches it.
    Available,
    /// The repository does not hold the commit at all.
    Missing,
    /// The repository holds the commit, but no fetched ref reaches it, so a
    /// clone of this repository cannot check it out.
    Unreachable,
}

impl CommitAvailability {
    /// Whether the commit can be checked out.
    pub fn is_available(self) -> bool {
        matches!(self, CommitAvailability::Available)
    }
}

/// Probe the storage at `git_path` for `commit`.
///
/// Availability is decided by the same rule the workspace clone obeys: the
/// commit must be reachable from a branch or tag, because a clone fetches
/// `refs/heads/*` and `refs/tags/*` and nothing else. A commit parked
/// under another namespace is present in the object store and still
/// unreachable at checkout, so object existence alone is not enough.
pub async fn probe(
    git_path: &Path,
    commit: &str,
    budget: Duration,
) -> anyhow::Result<CommitAvailability> {
    let revision = format!("{commit}^{{commit}}");
    let exists = crate::run_git_output(
        git_command(git_path, &["cat-file", "-e", &revision]),
        "commit presence",
        budget,
    )
    .await?;
    if !exists.status.success() {
        return Ok(CommitAvailability::Missing);
    }
    let containing = crate::run_git_output(
        git_command(
            git_path,
            &[
                "for-each-ref",
                "--contains",
                &revision,
                "--format=%(refname)",
                "refs/heads",
                "refs/tags",
            ],
        ),
        "commit reachability",
        budget,
    )
    .await?;
    if !containing.status.success() {
        // `for-each-ref` refuses a revision it cannot resolve. The object
        // exists (the presence check said so), so this is a reachability
        // question the probe cannot answer: treat it as unreachable rather
        // than as an error, so no run is created for a commit the clone
        // cannot reach.
        return Ok(CommitAvailability::Unreachable);
    }
    let refs = String::from_utf8_lossy(&containing.stdout);
    if refs.lines().any(|line| !line.trim().is_empty()) {
        Ok(CommitAvailability::Available)
    } else {
        Ok(CommitAvailability::Unreachable)
    }
}

/// A `git` invocation rooted at `git_path`.
fn git_command(git_path: &Path, args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command.arg("-C").arg(git_path).args(args);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

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

    /// A bare repository with one branch and one commit.
    fn seed_repository(root: &Path) -> (std::path::PathBuf, String) {
        std::fs::create_dir_all(root).expect("create seed root");
        let bare = root.join("probe.git");
        let seed = root.join("seed");
        run_git(
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                bare.to_str().expect("bare path"),
            ],
            root,
        );
        std::fs::create_dir_all(&seed).expect("create seed dir");
        run_git(&["init", "--initial-branch=main"], &seed);
        run_git(&["config", "user.email", "probe@example.com"], &seed);
        run_git(&["config", "user.name", "Probe"], &seed);
        std::fs::write(seed.join("file.txt"), "content").expect("write file");
        run_git(&["add", "."], &seed);
        run_git(&["commit", "-m", "probe commit"], &seed);
        run_git(
            &["remote", "add", "origin", bare.to_str().expect("bare path")],
            &seed,
        );
        run_git(&["push", "origin", "main"], &seed);
        let head = run_git(&["rev-parse", "HEAD"], &seed);
        (bare, head)
    }

    #[tokio::test]
    async fn a_commit_on_a_branch_is_available() {
        let root = std::env::temp_dir().join(format!("gf-probe-{}", uuid::Uuid::new_v4()));
        let (bare, head) = seed_repository(&root);
        let availability = probe(&bare, &head, Duration::from_secs(30))
            .await
            .expect("probe");
        assert_eq!(availability, CommitAvailability::Available);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_unknown_commit_is_missing() {
        let root = std::env::temp_dir().join(format!("gf-probe-{}", uuid::Uuid::new_v4()));
        let (bare, _) = seed_repository(&root);
        let availability = probe(&bare, &"deadbeef".repeat(5), Duration::from_secs(30))
            .await
            .expect("probe");
        assert_eq!(availability, CommitAvailability::Missing);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_commit_only_a_non_fetched_namespace_reaches_is_unreachable() {
        let root = std::env::temp_dir().join(format!("gf-probe-{}", uuid::Uuid::new_v4()));
        let (bare, head) = seed_repository(&root);
        // Park the commit under a namespace a clone does not fetch. The
        // object stays in the store, so a presence check alone would call it
        // available — and the clone would still fail to check it out.
        run_git(&["update-ref", "refs/ci/probe", &head], &bare);
        run_git(&["update-ref", "-d", "refs/heads/main"], &bare);
        let availability = probe(&bare, &head, Duration::from_secs(30))
            .await
            .expect("probe");
        assert_eq!(availability, CommitAvailability::Unreachable);
        let _ = std::fs::remove_dir_all(&root);
    }
}
