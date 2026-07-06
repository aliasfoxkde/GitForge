#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Git hooks helper library
# Provides functions for installing, validating, and testing git hooks
# ─────────────────────────────────────────────────────────────────────────────
set -Eeuo pipefail

# ─── Colors ───────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
INFO()  { echo -e "${GREEN}[git-hook]${NC} $*"; }
WARN()  { echo -e "${YELLOW}[git-hook]${NC} $*"; }
ERROR() { echo -e "${RED}[git-hook]${NC} $*"; }

# ─── Hook Installation ────────────────────────────────────────────────────────
hooks_install() {
    local hooks_dir="${1:-$PWD/.githooks}"
    local git_root
    git_root=$(git rev-parse --show-toplevel) || {
        ERROR "Not in a git repository"
        return 1
    }

    INFO "Installing git hooks from: $hooks_dir"

    # Verify hooks directory exists and contains valid hooks
    if [[ ! -d "$hooks_dir" ]]; then
        ERROR "Hooks directory not found: $hooks_dir"
        return 1
    fi

    # Set git hooks path
    git config core.hooksPath "$git_root/$hooks_dir"
    local current_path
    current_path=$(git config core.hooksPath)
    INFO "core.hooksPath set to: $current_path"

    # Verify hooks are executable
    local hook_file
    for hook_file in "$hooks_dir"/pre-* "$hooks_dir"/post-*; do
        [[ -f "$hook_file" ]] || continue
        if [[ ! -x "$hook_file" ]]; then
            chmod +x "$hook_file"
            INFO "Made executable: $(basename "$hook_file")"
        fi
    done

    INFO "Git hooks installed successfully"
}

hooks_verify() {
    local git_root
    git_root=$(git rev-parse --show-toplevel) || {
        ERROR "Not in a git repository"
        return 1
    }

    local hooks_path
    hooks_path=$(git config core.hooksPath)

    if [[ -z "$hooks_path" ]]; then
        WARN "No custom hooks path configured (core.hooksPath not set)"
        return 1
    fi

    INFO "Configured hooks path: $hooks_path"

    # Check each standard hook
    local hook_name
    for hook_name in pre-commit pre-push pre-rebase; do
        local hook_file="$git_root/$hooks_path/$hook_name"
        if [[ -f "$hook_file" ]]; then
            INFO "✓ $hook_name exists"
        else
            WARN "✗ $hook_name not found"
        fi
    done
}

hooks_list() {
    local git_root
    git_root=$(git rev-parse --show-toplevel)
    local hooks_path
    hooks_path=$(git config core.hooksPath)

    if [[ -z "$hooks_path" ]]; then
        echo "No custom hooks configured"
        return
    fi

    echo "Hooks path: $hooks_path"
    echo ""
    ls -la "$git_root/$hooks_path/" 2>/dev/null || echo "(empty or not found)"
}

# ─── Hook Testing ─────────────────────────────────────────────────────────────
hooks_test() {
    local hook_name="${1:-pre-commit}"
    local git_root
    git_root=$(git rev-parse --show-toplevel) || return 1

    local hooks_path
    hooks_path=$(git config core.hooksPath)
    [[ -z "$hooks_path" ]] && hooks_path=".githooks"

    local hook_file="$git_root/$hooks_path/$hook_name"

    if [[ ! -f "$hook_file" ]]; then
        ERROR "Hook not found: $hook_name"
        return 1
    fi

    INFO "Testing $hook_name..."

    # Run hook in dry-run mode if supported
    if grep -q 'dry.run\|DRY_RUN\|--dry-run' "$hook_file" 2>/dev/null; then
        DRY_RUN=1 "$hook_file" --dry-run 2>&1 || true
    else
        # Just check syntax
        bash -n "$hook_file" && INFO "Syntax OK" || {
            ERROR "Syntax error in $hook_name"
            return 1
        }
    fi
}
