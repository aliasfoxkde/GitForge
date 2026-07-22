#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Pre-commit Hooks — Python
# Runs: ruff check, ruff format, mypy (if configured)
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/common.sh"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/staged-files.sh"

INFO "Checking Python..."

# ─── Check for Python files ─────────────────────────────────────────────────
STAGED_PY=$(get_staged_python_files)
if [ -z "$STAGED_PY" ]; then
    DEBUG "No Python files staged — skipping Python checks"
    exit 0
fi

# ─── Check if uv is available ───────────────────────────────────────────────
PYTHON_TOOL="${PYTHON_TOOL:-uv}"
if ! command -v "$PYTHON_TOOL" &>/dev/null; then
    WARN "$PYTHON_TOOL not found — skipping Python checks"
    exit 0
fi

# ─── ruff check ─────────────────────────────────────────────────────────────
INFO "Running ruff check..."
if ! "$PYTHON_TOOL" run ruff check . --output-format=github 2>&1; then
    ERROR "ruff check failed"
    block_on_error "ruff check failed"
fi

# ─── ruff format ────────────────────────────────────────────────────────────
INFO "Checking ruff format..."
if ! "$PYTHON_TOOL" run ruff format --check . 2>&1; then
    ERROR "ruff format must format these files. Run: uv run ruff format ."
    block_on_error "ruff format failed"
fi

# ─── mypy (optional — only if configured) ──────────────────────────────────
if [ -f "pyproject.toml" ] && grep -q '\[tool.mypy\]' pyproject.toml 2>/dev/null; then
    INFO "Running mypy type check..."
    if ! "$PYTHON_TOOL" run mypy src/ --no-error-summary 2>&1; then
        WARN "mypy type check reported issues — review above"
    fi
fi

INFO "Python checks passed"
exit 0
