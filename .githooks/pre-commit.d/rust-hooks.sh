#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Pre-commit Hooks — Rust
# Runs: cargo fmt --check, clippy, cargo test (staged crates)
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/common.sh"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/staged-files.sh"

INFO "Checking Rust..."

# ─── Check for Rust files ───────────────────────────────────────────────────
STAGED_RS=$(get_staged_rust_files)
if [ -z "$STAGED_RS" ]; then
    DEBUG "No Rust files staged — skipping Rust checks"
    exit 0
fi

# ─── cargo fmt ─────────────────────────────────────────────────────────────
INFO "Checking cargo fmt..."
if ! cargo fmt --all -- --check 2>&1; then
    ERROR "cargo fmt must format these files. Run: cargo fmt --all"
    block_on_error "cargo fmt failed"
fi

# ─── clippy ─────────────────────────────────────────────────────────────────
INFO "Running clippy..."
# Only check staged files to avoid rebuilding entire workspace
STAGED_DIRS=$(get_staged_rust_dirs | tr '\n' ' ')
if [ -n "$STAGED_DIRS" ]; then
    # shellcheck disable=SC2086
    if ! cargo clippy --all-targets --all-features -- -D warnings 2>&1; then
        ERROR "clippy reported errors"
        block_on_error "clippy failed"
    fi
fi

# ─── cargo test (staged crates only) ────────────────────────────────────────
INFO "Running cargo test for staged crates..."
STAGED_DIRS=$(get_staged_rust_dirs | tr '\n' ' ')
if [ -n "$STAGED_DIRS" ]; then
    # Run tests only for crates with staged changes
    for dir in $STAGED_DIRS; do
        # Find the closest Cargo.toml to this directory
        cargo_toml=$(find "$dir" -maxdepth 3 -name "Cargo.toml" | head -1)
        if [ -n "$cargo_toml" ]; then
            crate_dir=$(dirname "$cargo_toml")
            INFO "Testing crate: $crate_dir"
            # shellcheck disable=SC2086
            if ! cargo test --manifest-path="$cargo_toml" 2>&1; then
                ERROR "cargo test failed for $crate_dir"
                block_on_error "cargo test failed"
            fi
        fi
    done
fi

INFO "Rust checks passed"
exit 0
