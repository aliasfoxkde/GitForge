#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory pre-push Hooks — Go
# Runs: full test suite with coverage gate
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/common.sh"

INFO "Running Go pre-push hooks..."

# ─── Full test suite ───────────────────────────────────────────────────────
COVERAGE_THRESHOLD="${COVERAGE_THRESHOLD:-70}"

INFO "Running full test suite..."
COVERAGE_OUT=$(mktemp)
if ! go test -p 1 -timeout 15m -coverprofile="$COVERAGE_OUT" ./... 2>&1; then
    ERROR "Tests failed"
    rm -f "$COVERAGE_OUT"
    exit 1
fi

COV=$(go tool cover -func="$COVERAGE_OUT" | grep total | awk '{print $3}' | tr -d '%')
rm -f "$COVERAGE_OUT"

INFO "Coverage: ${COV}%"
if (( $(echo "$COV < $COVERAGE_THRESHOLD" | bc -l) )); then
    ERROR "Coverage ${COV}% is below threshold ${COVERAGE_THRESHOLD}%"
    exit 1
fi

INFO "Go pre-push hooks passed"
exit 0
