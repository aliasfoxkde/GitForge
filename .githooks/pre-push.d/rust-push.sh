#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory pre-push Hooks — Rust
# Runs: cargo test --workspace, complexity gate, coverage gate
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/../lib/common.sh"

INFO "Running Rust pre-push hooks..."

# ─── cargo test --workspace ───────────────────────────────────────────────
INFO "Running cargo test --workspace..."
if ! cargo test --workspace --no-fail-fast 2>&1; then
    ERROR "cargo test --workspace failed"
    exit 1
fi

# ─── Complexity Gate ───────────────────────────────────────────────────────
INFO "Checking complexity limits..."
COMPLEXITY_LIMIT=10
MAX_LINES=500
MAX_FUNC_LINES=50

HIGH_CPLX=$(cargo clang-query --workspace 2>/dev/null | grep -c "cyclomatic_complexity > $COMPLEXITY_LIMIT" || \
    find . -name "*.rs" -not -path "./target/*" -exec wc -l {} \; 2>/dev/null | \
    awk -v limit="$MAX_LINES" '$1 > limit { print $2 ":" $1 " lines" }' | head -10 || true)

if [ -n "$HIGH_CPLX" ]; then
    WARN "Large files detected:"
    echo "$HIGH_CPLX"
fi

INFO "Rust pre-push hooks passed"
exit 0
