#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Git Hooks — Language Detection
# Detects which languages are present in the repository
# ─────────────────────────────────────────────────────────────────────────────

# Source common utilities first
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/common.sh"

# ─── Detect Go ──────────────────────────────────────────────────────────────
detect_go() {
    [ -f "$REPO_ROOT/go.mod" ]
}

# ─── Detect Python ─────────────────────────────────────────────────────────
detect_python() {
    if [ ! -f "$REPO_ROOT/pyproject.toml" ]; then
        return 1
    fi
    # Must have actual Python source files
    local py_files
    py_files=$(find "$REPO_ROOT" \
        -name "*.py" \
        -not -path "$REPO_ROOT/template-parts/*" \
        -not -path "$REPO_ROOT/.github/*" \
        -not -path "$REPO_ROOT/docs/*" \
        -not -path "$REPO_ROOT/.git/*" \
        -not -path "$REPO_ROOT/venv/*" \
        -not -path "$REPO_ROOT/.venv/*" \
        -not -path "$REPO_ROOT/__pycache__/*" \
        2>/dev/null | head -1)
    [ -n "$py_files" ]
}

# ─── Detect Rust ────────────────────────────────────────────────────────────
detect_rust() {
    [ -f "$REPO_ROOT/Cargo.toml" ]
}

# ─── Detect TypeScript ───────────────────────────────────────────────────────
detect_typescript() {
    if [ ! -f "$REPO_ROOT/package.json" ]; then
        return 1
    fi
    local ts_files
    ts_files=$(find "$REPO_ROOT" \
        \( -name "*.ts" -o -name "*.tsx" \) \
        -not -path "$REPO_ROOT/template-parts/*" \
        -not -path "$REPO_ROOT/node_modules/*" \
        -not -path "$REPO_ROOT/.github/*" \
        -not -path "$REPO_ROOT/docs/*" \
        -not -path "$REPO_ROOT/.git/*" \
        2>/dev/null | head -1)
    [ -n "$ts_files" ]
}

# ─── Detect Bash ────────────────────────────────────────────────────────────
detect_bash() {
    [ -f "$REPO_ROOT/.shellcheckrc" ] && return 0
    local sh_files
    sh_files=$(find "$REPO_ROOT" \
        -name "*.sh" \
        -not -path "$REPO_ROOT/.github/workflows/*" \
        -not -path "$REPO_ROOT/.git/*" \
        -not -path "$REPO_ROOT/template-parts/*" \
        -not -path "$REPO_ROOT/node_modules/*" \
        -not -path "$REPO_ROOT/.githooks/*" \
        2>/dev/null | head -1)
    [ -n "$sh_files" ]
}

# ─── Get all detected languages ────────────────────────────────────────────
get_detected_languages() {
    local langs=()
    detect_go    && langs+=("go")
    detect_python && langs+=("python")
    detect_rust  && langs+=("rust")
    detect_typescript && langs+=("typescript")
    detect_bash  && langs+=("bash")
    echo "${langs[@]}"
}
