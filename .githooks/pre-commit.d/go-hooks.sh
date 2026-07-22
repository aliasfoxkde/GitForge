#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Pre-commit Hooks — Go
# Runs: gofmt, goimports, go vet, go test
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/common.sh"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/staged-files.sh"

INFO "Checking Go..."

# ─── Check for Go files ────────────────────────────────────────────────────
STAGED_GO=$(get_staged_go_files)
if [ -z "$STAGED_GO" ]; then
    DEBUG "No Go files staged — skipping Go checks"
    exit 0
fi

# ─── gofmt ─────────────────────────────────────────────────────────────────
INFO "Checking gofmt..."
UNFORMATTED=$(gofmt -l .)
if [ -n "$UNFORMATTED" ]; then
    ERROR "gofmt must format these files:"
    echo "$UNFORMATTED"
    ERROR "Run: gofmt -w ."
    block_on_error "gofmt failed"
fi

# ─── goimports ─────────────────────────────────────────────────────────────
if command -v goimports &>/dev/null; then
    INFO "Checking goimports..."
    UNIMPORTED=$(goimports -l .)
    if [ -n "$UNIMPORTED" ]; then
        ERROR "goimports must format these files:"
        echo "$UNIMPORTED"
        block_on_error "goimports failed"
    fi
fi

# ─── go vet ─────────────────────────────────────────────────────────────────
INFO "Running go vet..."
if ! go vet ./... 2>&1; then
    ERROR "go vet failed"
    block_on_error "go vet failed"
fi

# ─── go test (staged packages only) ───────────────────────────────────────
STAGED_DIRS=$(get_staged_go_dirs)
if [ -z "$STAGED_DIRS" ]; then
    DEBUG "No Go directories found for testing"
    exit 0
fi

INFO "Running tests for staged packages..."
# shellcheck disable=SC2086
if ! go test -p 1 -timeout 5m $STAGED_DIRS 2>&1; then
    ERROR "Tests failed for staged packages"
    block_on_error "go test failed"
fi

INFO "Go checks passed"
exit 0
