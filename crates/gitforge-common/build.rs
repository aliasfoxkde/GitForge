//! Build-time provenance stamp.
//!
//! The release chain has to be able to prove that the binaries it ships were
//! built from the commit the bundle claims. The release binaries live in
//! gitignored paths, so a bundle assembled from stale leftovers looks
//! exactly like a fresh one to every other gate: the only honest link
//! between "this bundle claims commit X" and "these bytes were built from
//! commit X" has to be carried by the binaries themselves.
//!
//! The stamp is resolved in this order:
//!
//! 1. `GITFORGE_BUILD_COMMIT` — an explicit override, for builds that happen
//!    outside a git checkout (a source copy on a build host, for example).
//! 2. `git rev-parse HEAD` — the checkout this crate is built from.
//! 3. `unknown` — no provenance is available; the release tooling refuses
//!    to ship a binary that cannot name its commit.
//!
//! The state of the tree is stamped next to the commit: a binary built from
//! a modified working tree does not correspond to the commit it claims, so
//! `dirty` records whether the tree was clean at build time.

use std::path::PathBuf;
use std::process::Command;

/// The value stamped when no commit could be determined.
const UNKNOWN: &str = "unknown";

fn main() {
    println!("cargo:rerun-if-env-changed=GITFORGE_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=GITFORGE_BUILD_DIRTY");
    watch_checkout();
    let commit = resolve_commit();
    let dirty = resolve_dirty();
    println!("cargo:rustc-env=GITFORGE_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=GITFORGE_BUILD_DIRTY={dirty}");
}

/// Run `git` and return its trimmed stdout, or `None` when git is missing,
/// the command fails, or its output is not valid UTF-8.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    Some(stdout.trim().to_string())
}

/// The commit the binaries are being built from.
fn resolve_commit() -> String {
    if let Ok(value) = std::env::var("GITFORGE_BUILD_COMMIT") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    git(&["rev-parse", "HEAD"])
        .filter(|commit| !commit.is_empty())
        .unwrap_or_else(|| UNKNOWN.to_string())
}

/// Whether the working tree carried uncommitted changes at build time:
/// `false`, `true`, or `unknown` when git could not answer.
///
/// An explicit `GITFORGE_BUILD_DIRTY` overrides the answer. A build from a
/// source copy has no checkout to ask, and the operator asserting the state
/// of the copy is better than the release tooling refusing every such build
/// without a way forward.
fn resolve_dirty() -> String {
    if let Ok(value) = std::env::var("GITFORGE_BUILD_DIRTY") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    match git(&["status", "--porcelain", "--untracked-files=no"]) {
        Some(output) if output.is_empty() => "false".to_string(),
        Some(_) => "true".to_string(),
        None => UNKNOWN.to_string(),
    }
}

/// Tell cargo to re-run this build script when the checkout moves.
///
/// Without this, a build script that resolves the commit from git keeps its
/// first answer: switching branches or advancing the current branch does not
/// touch any file in the package, so cargo would happily relink binaries
/// that still claim the previous commit. That is the exact failure this
/// stamp exists to prevent.
fn watch_checkout() {
    let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) else {
        return;
    };
    let git_dir = PathBuf::from(git_dir);
    let head_path = git_dir.join("HEAD");
    if !head_path.exists() {
        return;
    }
    println!("cargo:rerun-if-changed={}", head_path.display());

    // A branch that advances does not change HEAD's own bytes, so the ref
    // HEAD points at has to be watched as well.
    let Some(common_dir) = git(&["rev-parse", "--git-common-dir"]) else {
        return;
    };
    let common_dir = absolute_path(&common_dir);
    let Ok(head_contents) = std::fs::read_to_string(&head_path) else {
        return;
    };
    let Some(reference) = head_contents.trim().strip_prefix("ref: ") else {
        // Detached HEAD: the commit lives in HEAD itself, which is watched.
        return;
    };
    let ref_path = common_dir.join(reference.trim());
    if ref_path.exists() {
        println!("cargo:rerun-if-changed={}", ref_path.display());
        return;
    }
    // The ref is packed rather than loose; packed-refs carries it.
    let packed_refs = common_dir.join("packed-refs");
    if packed_refs.exists() {
        println!("cargo:rerun-if-changed={}", packed_refs.display());
    }
}

/// Resolve a possibly relative path against the current directory.
fn absolute_path(path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        return candidate;
    }
    match std::env::current_dir() {
        Ok(current) => current.join(candidate),
        Err(_) => candidate,
    }
}
