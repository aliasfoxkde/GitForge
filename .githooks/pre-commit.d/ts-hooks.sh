#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Pre-commit Hooks — TypeScript/JavaScript
# Runs: eslint, prettier check, vitest (staged files)
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/common.sh"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/staged-files.sh"

INFO "Checking TypeScript/JavaScript..."

# ─── Check for TypeScript files ─────────────────────────────────────────────
STAGED_TS=$(get_staged_ts_files)
STAGED_JS=$(get_staged_js_files)
if [ -z "$STAGED_TS" ] && [ -z "$STAGED_JS" ]; then
    DEBUG "No TypeScript/JavaScript files staged — skipping TS/JS checks"
    exit 0
fi

# ─── Check if npm is available ─────────────────────────────────────────────
if ! command -v npm &>/dev/null; then
    WARN "npm not found — skipping TypeScript checks"
    exit 0
fi

# ─── eslint ─────────────────────────────────────────────────────────────────
if [ -f ".eslintrc.json" ] || [ -f ".eslintrc.js" ] || [ -f "eslint.config.js" ]; then
    INFO "Running ESLint..."
    if ! npx eslint . --max-warnings 0 2>&1; then
        ERROR "ESLint reported errors"
        block_on_error "ESLint failed"
    fi
fi

# ─── prettier ───────────────────────────────────────────────────────────────
if [ -f ".prettierrc" ] || [ -f ".prettierrc.json" ] || [ -f ".prettierrc.js" ]; then
    INFO "Checking Prettier..."
    if ! npx prettier --check . 2>&1; then
        ERROR "Prettier formatting issues found. Run: npx prettier --write ."
        block_on_error "Prettier check failed"
    fi
fi

# ─── vitest (staged tests only) ─────────────────────────────────────────────
STAGED_TESTS=$(echo "$STAGED_TS" | grep -E '\.(test|spec)\.(ts|tsx|js|jsx)$' || true)
if [ -n "$STAGED_TESTS" ]; then
    INFO "Running vitest for staged test files..."
    if ! npx vitest run --reporter=verbose 2>&1; then
        ERROR "vitest reported test failures"
        block_on_error "vitest failed"
    fi
fi

INFO "TypeScript/JavaScript checks passed"
exit 0
