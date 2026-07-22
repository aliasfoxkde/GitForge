#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory pre-push Hooks — TypeScript
# Runs: vitest with coverage gate
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/common.sh"

INFO "Running TypeScript pre-push hooks..."

if ! command -v npm &>/dev/null; then
    WARN "npm not found — skipping TypeScript pre-push checks"
    exit 0
fi

# ─── vitest with coverage ──────────────────────────────────────────────────
INFO "Running vitest with coverage..."
if ! npx vitest run --coverage 2>&1; then
    ERROR "vitest failed"
    exit 1
fi

INFO "TypeScript pre-push hooks passed"
exit 0
