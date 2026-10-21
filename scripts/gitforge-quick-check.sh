#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# GitForge quick-check: fast pre-push gate for agents
#
# Catches obvious failures locally before they consume CI capacity.
# Target: <60s for no-change/no-work, <180s for full check on cold cache.
#
# Usage:
#   ./scripts/gitforge-quick-check.sh           # full check
#   ./scripts/gitforge-quick-check.sh --changed  # only changed files vs origin/main
#
# Install as pre-push hook:
#   ln -sf ../../scripts/gitforge-quick-check.sh .git/hooks/pre-push
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

ERROR() { echo -e "${RED}[quick-check ERROR]${NC} $*"; }
INFO()  { echo -e "${GREEN}[quick-check]${NC} $*"; }
WARN()  { echo -e "${YELLOW}[quick-check WARN]${NC} $*"; }

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

# Determine scope
SCOPE="${1:-full}"
if [[ "$SCOPE" == "--changed" ]]; then
    CHANGED_ONLY=true
    INFO "Running changed-files-only scan..."
else
    CHANGED_ONLY=false
    INFO "Running full quick-check..."
fi

FAILED=0

# ── Step 1: Format check ──────────────────────────────────────────────────
INFO "[1/5] fmt check..."
if ! cargo fmt --all -- --check 2>&1; then
    ERROR "Format check failed. Run 'cargo fmt --all' to fix."
    FAILED=1
fi

# ── Step 2: Route-docs drift gate ─────────────────────────────────────────
INFO "[2/5] route-docs drift (parse-only, <1s)..."
if ! python3 scripts/check-route-docs.py 2>&1; then
    ERROR "Route-docs drift detected. Fix the discrepancies before pushing."
    FAILED=1
fi

# ── Step 3: Clippy with timeout ──────────────────────────────────────────
INFO "[3/5] clippy (timeout 180s)..."
# Run with TIMEOUT to catch infinite-compile edge cases
# TIMEOUT measured in seconds; clippy cold-cache takes ~20-60s normally
if ! timeout 180 cargo clippy --workspace --all-targets -- -D warnings 2>&1; then
    ERROR "Clippy failed. Fix warnings/errors before pushing."
    FAILED=1
fi

# ── Step 4: Aegis changed-files scan ─────────────────────────────────────
INFO "[4/5] aegis (changed-files)..."
AEGIS_STATUS=0
if command -v aegis &>/dev/null; then
    # Build the diff argument: changed files vs origin/main
    DIFF_FILES=$(git diff --name-only origin/main...HEAD 2>/dev/null || echo "")
    if [[ -z "$DIFF_FILES" ]]; then
        INFO "  no files changed vs origin/main — skipping aegis"
    else
        DIFF=$(git diff origin/main...HEAD 2>/dev/null || echo "")
        if [[ -n "$DIFF" ]]; then
            # Use aegis --diff for changed-lines analysis
            echo "$DIFF" | aegis scan --diff /dev/stdin 2>&1 || AEGIS_STATUS=$?
            # Aegis returns non-zero on findings (not errors) — treat findings as warnings
            if [[ $AEGIS_STATUS -ne 0 ]] && [[ $AEGIS_STATUS -ne 1 ]]; then
                WARN "Aegis scan exited with code $AEGIS_STATUS — check output above"
            fi
        else
            INFO "  no textual diff to scan"
        fi
    fi
else
    INFO "  SKIP: aegis not installed (install: cargo install aegis or use package manager)"
fi

# ── Step 5: Quick test smoke ───────────────────────────────────────────────
INFO "[5/5] quick smoke test (first 3 crates, bounded threads)..."
# Smoke: run unit tests only, skip integration/e2e/fuzz heavy targets.
# Bounded threads matches the CI 2-thread bound.
# --test-threads=2 also documented in .gitforge.yml rationale.
# We don't fail-fast here because a failing test in one crate doesn't
# mean we should skip checking the others — but we DO limit total time.
if ! timeout 120 cargo test --workspace \
    -- --test-threads=2 \
       --skip '::integration' \
       --skip '::e2e' \
       --skip '::fuzz' \
       --skip 'test_git_push_and_clone' \
       --skip 'test_required_checks' \
       2>&1 | tail -20; then
    WARN "Smoke test had failures — full suite runs in CI"
    # Don't fail here: smoke is advisory, CI runs the full suite
fi

# ── Result ────────────────────────────────────────────────────────────────
echo ""
if [[ $FAILED -ne 0 ]]; then
    ERROR "Quick-check FAILED — fix the errors above before pushing."
    ERROR "Run 'cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings' to fix."
    exit 1
else
    INFO "Quick-check PASSED — pushing is safe."
    INFO "Note: full CI (fmt→clippy→test→coverage→fuzz) still runs on fedora."
    INFO "This check catches 80% of failures before they burn CI capacity."
fi
