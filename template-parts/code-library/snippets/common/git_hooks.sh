#!/bin/bash
# git_hooks.sh - Git hooks installation and validation helpers
# Part of Dark Factory code-library snippets

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Log functions
log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Check if git hooks are installed
check_hooks_installed() {
    if [[ -f ".git/hooks/pre-commit" ]] || [[ -L ".git/hooks/pre-commit" ]]; then
        log_info "Git hooks are installed"
        return 0
    else
        log_warn "Git hooks are NOT installed"
        return 1
    fi
}

# Install git hooks from .githooks directory
install_hooks() {
    local hooks_dir="${1:-.githooks}"
    local git_hooks_dir=".git/hooks"

    if [[ ! -d "$hooks_dir" ]]; then
        log_error "Hooks directory not found: $hooks_dir"
        return 1
    fi

    # Check if hooks are already symlinked
    if [[ "$(git config core.hooksPath)" == "$hooks_dir" ]]; then
        log_info "Hooks are already configured via git config"
        return 0
    fi

    # Option 1: Symlink hooks directory
    log_info "Setting up hooks via git config..."
    git config core.hooksPath "$hooks_dir"
    log_info "Hooks installed successfully"

    # Option 2: Copy hooks (alternative)
    # cp -r "$hooks_dir/"* "$git_hooks_dir/"
}

# Validate a single hook
validate_hook() {
    local hook_path="$1"
    local hook_name=$(basename "$hook_path")

    if [[ ! -f "$hook_path" ]]; then
        log_error "Hook not found: $hook_name"
        return 1
    fi

    if [[ ! -x "$hook_path" ]]; then
        log_warn "Hook is not executable: $hook_name"
        log_info "Making executable: $hook_name"
        chmod +x "$hook_path"
    fi

    # Check for common issues
    if grep -q "/bin/bash -e" "$hook_path" 2>/dev/null; then
        log_warn "Hook uses 'set -e' which may exit on first error"
    fi

    log_info "Hook validated: $hook_name"
}

# Validate all hooks in directory
validate_hooks() {
    local hooks_dir="${1:-.githooks}"
    local failed=0

    if [[ ! -d "$hooks_dir" ]]; then
        log_error "Hooks directory not found: $hooks_dir"
        return 1
    fi

    log_info "Validating hooks in $hooks_dir..."

    for hook in "$hooks_dir"/*; do
        if [[ -f "$hook" ]]; then
            if ! validate_hook "$hook"; then
                ((failed++))
            fi
        fi
    done

    if [[ $failed -eq 0 ]]; then
        log_info "All hooks validated successfully"
        return 0
    else
        log_error "$failed hook(s) failed validation"
        return 1
    fi
}

# Run a specific hook manually
run_hook() {
    local hook_name="$1"
    local hooks_dir="${2:-.githooks}"
    local hook_path="$hooks_dir/$hook_name"

    if [[ ! -f "$hook_path" ]]; then
        log_error "Hook not found: $hook_name"
        return 1
    fi

    log_info "Running hook: $hook_name"
    "$hook_path"
}

# List available hooks
list_hooks() {
    local hooks_dir="${1:-.githooks}"

    if [[ ! -d "$hooks_dir" ]]; then
        log_error "Hooks directory not found: $hooks_dir"
        return 1
    fi

    log_info "Available hooks in $hooks_dir:"
    for hook in "$hooks_dir"/*; do
        if [[ -f "$hook" ]]; then
            local name=$(basename "$hook")
            local size=$(du -h "$hook" | cut -f1)
            echo "  - $name ($size)"
        fi
    done
}

# Main entry point
main() {
    local command="${1:-check}"

    case "$command" in
        check)
            check_hooks_installed
            ;;
        install)
            install_hooks "${2:-.githooks}"
            ;;
        validate)
            validate_hooks "${2:-.githooks}"
            ;;
        list)
            list_hooks "${2:-.githooks}"
            ;;
        run)
            run_hook "${2:-pre-commit}" "${3:-.githooks}"
            ;;
        *)
            echo "Usage: $0 {check|install|validate|list|run} [hook_name] [hooks_dir]"
            exit 1
            ;;
    esac
}

main "$@"
