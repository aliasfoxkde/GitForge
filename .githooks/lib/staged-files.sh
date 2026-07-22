#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Git Hooks — Staged Files Utilities
# Functions for working with staged files
# ─────────────────────────────────────────────────────────────────────────────

# Source common utilities
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "${SCRIPT_DIR}/common.sh"

# ─── Get all staged files ─────────────────────────────────────────────────
get_staged_files() {
    git diff --cached --name-only --diff-filter=ACM 2>/dev/null || true
}

# ─── Get staged files of a specific type ───────────────────────────────────
get_staged_files_by_ext() {
    local ext="$1"  # e.g., ".go", ".py", ".rs"
    get_staged_files | grep "$ext$" 2>/dev/null || true
}

# ─── Get staged Go files ───────────────────────────────────────────────────
get_staged_go_files() {
    get_staged_files_by_ext ".go"
}

# ─── Get staged Python files ─────────────────────────────────────────────────
get_staged_python_files() {
    get_staged_files_by_ext ".py"
}

# ─── Get staged Rust files ─────────────────────────────────────────────────
get_staged_rust_files() {
    get_staged_files_by_ext ".rs"
}

# ─── Get staged TypeScript files ───────────────────────────────────────────
get_staged_ts_files() {
    get_staged_files | grep -E '\.(ts|tsx)$' 2>/dev/null || true
}

# ─── Get staged JavaScript files ───────────────────────────────────────────
get_staged_js_files() {
    get_staged_files | grep -E '\.(js|jsx)$' 2>/dev/null || true
}

# ─── Get staged shell scripts ──────────────────────────────────────────────
get_staged_shell_files() {
    get_staged_files | grep -E '\.sh$' 2>/dev/null || true
}

# ─── Get staged Go files for a specific package ────────────────────────────
get_staged_go_dirs() {
    get_staged_go_files | xargs -I{} dirname {} 2>/dev/null | sort -u || true
}

# ─── Get staged Rust crate directories ─────────────────────────────────────
get_staged_rust_dirs() {
    get_staged_rust_files | xargs -I{} dirname {} 2>/dev/null | sort -u || true
}

# ─── Filter out non-language files ─────────────────────────────────────────
filter_text_files() {
    grep -vE '\.(png|jpg|gif|ico|svg|woff|woff2|ttf|eot|zip|tar|gz|bz2|xz|pdf|db|bin)$' 2>/dev/null || true
}

# ─── Check if any staged files exist ───────────────────────────────────────
has_staged_files() {
    [ -n "$(get_staged_files)" ]
}

# ─── Check if staged files match a pattern ─────────────────────────────────
staged_files_match() {
    local pattern="$1"
    get_staged_files | grep -qE "$pattern" 2>/dev/null
}
