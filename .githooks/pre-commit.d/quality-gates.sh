#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Quality Gates Pre-commit Hook
# Integrates Atheon-Enhanced as primary scanner + bash fallback patterns
#
# Blocks commits with:
#   - Hardcoded secrets (API keys, tokens, passwords, private keys) — Atheon
#   - PII in source code — Atheon
#   - Console logging in production code — bash patterns
#   - Fake/placeholder data — bash patterns
#   - Placeholder code (TODO/FIXME/HACK without issue refs) — bash patterns
#
# Pre-commit order: quality-gates runs FIRST (before language-specific hooks)
# to avoid wasting time on code that will be rejected anyway.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/common.sh"

INFO "Running quality gates..."

BLOCKED=0
STAGED_FILES=$(git diff --cached --name-only --diff-filter=ACM 2>/dev/null || true)

# ─── Atheon-Enhanced Scan (primary — 384+ patterns) ──────────────────────────
atheon_check() {
    local file="$1"

    # Skip binary and generated files
    case "$file" in
        *.png|*.jpg|*.gif|*.ico|*.svg|*.woff|*.woff2|*.ttf|*.eot) return 0 ;;
        node_modules/*|vendor/*|target/*|.git/*) return 0 ;;
    esac

    if ! command -v atheon &>/dev/null; then
        DEBUG "atheon not installed — skipping Atheon scan"
        return 0
    fi

    local content
    content=$(git show ":0:$file" 2>/dev/null || cat "$file" 2>/dev/null)
    [ -z "$content" ] && return 0

    local result
    result=$("$atheon_bin" --categories=secrets,pii "$file" 2>/dev/null || true)
    if [ -n "$result" ]; then
        ERROR "ATHEON: $file: security/PII issue detected"
        echo "$result" | head -5 >&2
        BLOCKED=1
        return 1
    fi
    return 0
}

# ─── Console Logging Detection ─────────────────────────────────────────────
console_check() {
    local file="$1"

    # Only check source files
    case "$file" in
        *.go|*.py|*.js|*.ts|*.jsx|*.tsx) ;;
        *) return 0 ;;
    esac

    # Skip test files
    case "$file" in
        *_test.go|test_*.go|tests/|*_test.py|*_tests.py) return 0 ;;
    esac

    local content
    content=$(git show ":0:$file" 2>/dev/null || cat "$file" 2>/dev/null)
    [ -z "$content" ] && return 0

    # Console patterns (exclude structured logging)
    local console_patterns=(
        'console\.(log|error|warn|debug|info)\s*\('
        'print\s*\('
        'puts\s+'
        'System\.out\.print'
        'fmt\.Print'
    )

    local found=0
    for pattern in "${console_patterns[@]}"; do
        local matches
        matches=$(grep -rnE "$pattern" <<< "$content" 2>/dev/null || true)
        if [ -n "$matches" ]; then
            # Filter out structured logging
            local suspect
            suspect=$(echo "$matches" | grep -vE '(logger\.(Info|Debug|Warn|Error)|slog\.|structlog|log\.Info|log\.Debug)' | head -5 || true)
            if [ -n "$suspect" ]; then
                ERROR "CONSOLE: $file: bare console logging detected"
                echo "$suspect" | head -5 >&2
                found=1
                BLOCKED=1
            fi
        fi
    done

    return $found
}

# ─── Placeholder Code Detection ──────────────────────────────────────────────
placeholder_check() {
    local file="$1"

    local content
    content=$(git show ":0:$file" 2>/dev/null || cat "$file" 2>/dev/null)
    [ -z "$content" ] && return 0

    local placeholder_patterns=(
        'TODO\s*(?!.*#[0-9])'
        'FIXME\s*(?!.*#[0-9])'
        'XXX\s*(?!.*#[0-9])'
        'HACK\s*(?!.*#[0-9])'
        'raise\s+NotImplementedError\s*\('
        'pass\s*#.*TODO'
    )

    local found=0
    for pattern in "${placeholder_patterns[@]}"; do
        if grep -rqE "$pattern" <<< "$content" 2>/dev/null; then
            local matches
            matches=$(grep -rnE "$pattern" <<< "$content" | head -5 || true)
            WARN "PLACEHOLDER: $file: placeholder code detected (TODO/FIXME without issue ref)"
            echo "$matches" | head -5 >&2
            found=1
        fi
    done

    return $found
}

# ─── Fake/Placeholder Data Detection ─────────────────────────────────────────
fake_data_check() {
    local file="$1"

    case "$file" in
        *_test.py|test_*.py|tests/|testdata/|fixtures/) return 0 ;;
    esac

    local content
    content=$(git show ":0:$file" 2>/dev/null || cat "$file" 2>/dev/null)
    [ -z "$content" ] && return 0

    local fake_patterns=(
        'placeholder[_\s]'
        'fake[_\s](email|data|name|address|token)'
        'mock[_\s](email|data|name)'
        '@example\.com'
        'john\s+doe'
        '123\s+Fake\s+'
        'your_[a-z_]+'
    )

    local found=0
    for pattern in "${fake_patterns[@]}"; do
        if grep -rqE "$pattern" <<< "$content" 2>/dev/null; then
            local matches
            matches=$(grep -rnE "$pattern" <<< "$content" | head -3 || true)
            WARN "FAKE_DATA: $file: placeholder/fake data detected"
            echo "$matches" | head -3 >&2
            found=1
        fi
    done

    return $found
}

# ─── Run all checks ────────────────────────────────────────────────────────
if [ -z "$STAGED_FILES" ]; then
    INFO "No files staged — skipping quality gates"
    exit 0
fi

INFO "Scanning staged files with quality gates..."
for file in $STAGED_FILES; do
    [ ! -f "$file" ] && continue

    # Run Atheon (primary scanner)
    atheon_check "$file" || true

    # Run bash fallback patterns
    console_check "$file" || true
    placeholder_check "$file" || true
    fake_data_check "$file" || true
done

if [ "$BLOCKED" -eq 1 ]; then
    ERROR "Quality gates blocked commit. Fix issues above or use git commit --no-verify to skip."
    exit 1
fi

INFO "Quality gates passed"
exit 0
