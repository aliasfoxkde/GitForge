//! Packaged route-docs drift checker.
//!
//! Thin wrapper: runs the repo's `scripts/check-route-docs.py` logic
//! against `GITFORGE_REPO_ROOT` (or the directory the binary is invoked
//! from, or its first argument). The checker stays a script so the CI
//! image needs no python packaging; this binary exists so projects that
//! consume `gitforge-testing-common` get the gate through cargo too.

// The crate-wide lint carve-out lives in lib.rs, which bins do not
// inherit — restate it here: a CLI tool that cannot find its inputs
// SHOULD exit with a panic message naming what is missing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let root = match std::env::args().nth(1) {
        Some(arg) => PathBuf::from(arg),
        None => match std::env::var("GITFORGE_REPO_ROOT") {
            Ok(path) => PathBuf::from(path),
            Err(_) => std::env::current_dir().unwrap_or_else(|e| {
                panic!(
                    "check-route-docs: no repo root argument, no GITFORGE_REPO_ROOT, and no cwd: {e}"
                )
            }),
        },
    };

    let script = root.join("scripts/check-route-docs.py");
    if !script.is_file() {
        panic!(
            "check-route-docs: no checker at {} (pass the repo root as the first argument or set GITFORGE_REPO_ROOT)",
            script.display()
        );
    }

    let status = Command::new("python3")
        .arg("-I")
        .arg(&script)
        .env("GITFORGE_REPO_ROOT", &root)
        .status()
        .unwrap_or_else(|e| panic!("check-route-docs: cannot run python3: {e}"));

    std::process::exit(status.code().unwrap_or(1));
}
