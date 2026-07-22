#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Git Hooks — Common Utilities
# Shared functions for all hook scripts
# ─────────────────────────────────────────────────────────────────────────────

# ─── Colors ──────────────────────────────────────────────────────────────────
export RED='\033[0;31m'
export GREEN='\033[0;32m'
export YELLOW='\033[1;33m'
export BLUE='\033[0;34m'
export NC='\033[0m' # No Color

# ─── Logging ────────────────────────────────────────────────────────────────
INFO()  { echo -e "${GREEN}[hook]${NC} $*"; }
WARN()  { echo -e "${YELLOW}[hook]${NC} $*"; }
ERROR() { echo -e "${RED}[hook]${NC} $*"; }
DEBUG() { [ -n "${DEBUG_HOOKS:-}" ] && echo -e "${BLUE}[hook:debug]${NC} $*" || true; }

# ─── Error Handling ──────────────────────────────────────────────────────────
HOOK_FAILED=0
block_on_error() {
    local msg="$1"
    ERROR "$msg"
    HOOK_FAILED=1
}

# ─── Repo Root ─────────────────────────────────────────────────────────────
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$REPO_ROOT" ]; then
    ERROR "Not in a git repository"
    exit 1
fi

# ─── Source guard ──────────────────────────────────────────────────────────
# Prevent double-sourcing
if [ -n "${_DARK_FACTORY_COMMON_SOURCED:-}" ]; then
    return 0
fi
export _DARK_FACTORY_COMMON_SOURCED=1

# ─── OS detection ───────────────────────────────────────────────────────────
is_macos() {
    [[ "$(uname -s)" == "Darwin" ]]
}

is_linux() {
    [[ "$(uname -s)" == "Linux" ]]
}

# ─── Required tool checks ───────────────────────────────────────────────────
require_tool() {
    local tool="$1"
    local package="${2:-$1}"
    if ! command -v "$tool" &>/dev/null; then
        WARN "$tool not found — install with: brew install $package (macOS) or apt install $package (Linux)"
        return 1
    fi
    return 0
}

# ─── Dry-run mode ──────────────────────────────────────────────────────────
# Set DRY_RUN=1 to preview without making changes
is_dry_run() {
    [ "${DRY_RUN:-0}" == "1" ]
}
