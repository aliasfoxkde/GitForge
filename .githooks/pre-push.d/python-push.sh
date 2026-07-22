#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory pre-push Hooks — Python
# Runs: pytest with coverage gate
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/common.sh"

INFO "Running Python pre-push hooks..."

PYTHON_TOOL="${PYTHON_TOOL:-uv}"
COVERAGE_THRESHOLD="${COVERAGE_THRESHOLD:-70}"

if ! command -v "$PYTHON_TOOL" &>/dev/null; then
    WARN "$PYTHON_TOOL not found — skipping Python pre-push checks"
    exit 0
fi

# ─── pytest with coverage ──────────────────────────────────────────────────
INFO "Running pytest with coverage..."
if ! "$PYTHON_TOOL" run pytest --timeout=60 -v --cov=. --cov-report=term-missing 2>&1; then
    ERROR "pytest failed"
    exit 1
fi

# ─── Coverage check ───────────────────────────────────────────────────────
COV=$("$PYTHON_TOOL" run coverage report --precision=2 2>/dev/null | grep total | awk '{print $4}' | tr -d '%' || echo "0")
INFO "Coverage: ${COV}%"
if (( $(echo "$COV < $COVERAGE_THRESHOLD" | bc -l) )); then
    ERROR "Coverage ${COV}% is below threshold ${COVERAGE_THRESHOLD}%"
    exit 1
fi

INFO "Python pre-push hooks passed"
exit 0
