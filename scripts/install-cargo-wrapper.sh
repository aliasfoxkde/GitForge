#!/bin/bash
# install-cargo-wrapper.sh - Install cargo-wrapper with alias for gitforge-build enforcement
#
# Usage: ./install-cargo-wrapper.sh [--uninstall] [--dry-run]
#
# This script:
#   1. Copies cargo-wrapper to ~/.cargo/bin/
#   2. Adds alias to ~/.bashrc (or specified shell rc)
#   3. Creates backup of original files
#
# WARNING: This does NOT replace the cargo binary (safer approach than mv/cp)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WRAPPER_SOURCE="${SCRIPT_DIR}/../.cargo/bin/cargo-wrapper"
BACKUP_DIR="${HOME}/.cargo/bin.backup/$(date +%Y%m%d_%H%M%S)"
DRY_RUN=false
UNINSTALL=false
SHELL_RC="${HOME}/.bashrc"

usage() {
    cat << EOF
Usage: $(basename "$0") [OPTIONS]

Install cargo-wrapper for gitforge-build enforcement.

OPTIONS:
    --uninstall     Uninstall (restore backup, remove alias)
    --dry-run       Show what would be done without making changes
    --shellrc FILE  Shell rc file to modify (default: ~/.bashrc)
    --help          Show this help

EXAMPLES:
    $(basename "$0")                  # Install
    $(basename "$0") --dry-run       # Preview changes
    $(basename "$0") --uninstall     # Restore original

EOF
    exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --uninstall) UNINSTALL=true; shift ;;
        --dry-run) DRY_RUN=true; shift ;;
        --shellrc) SHELL_RC="$2"; shift 2 ;;
        --help) usage ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

log() { echo "[install] $1"; }
warn() { echo "[install] WARNING: $1" >&2; }
die() { echo "[install] ERROR: $1" >&2; exit 1; }

# Find cargo-wrapper source
if [[ ! -f "$WRAPPER_SOURCE" ]]; then
    # Try current directory
    WRAPPER_SOURCE="$(which cargo-wrapper 2>/dev/null)" || true
fi

if [[ -z "$WRAPPER_SOURCE" ]] || [[ ! -f "$WRAPPER_SOURCE" ]]; then
    die "cargo-wrapper not found. Ensure this script is run from the GitForge repository root."
fi

ALIAS_LINE='# cargo-wrapper for gitforge-build enforcement (managed by install-cargo-wrapper.sh)'
ALIAS_COMMAND='alias cargo=~/.cargo/bin/cargo-wrapper'

uninstall() {
    log "Uninstalling cargo-wrapper..."

    # Remove alias from shell rc
    if [[ -f "$SHELL_RC" ]]; then
        if grep -q "$ALIAS_LINE" "$SHELL_RC" 2>/dev/null; then
            if [[ "$DRY_RUN" == "true" ]]; then
                log "[DRY-RUN] Would remove alias from $SHELL_RC"
            else
                # Remove alias lines
                sed -i "/${ALIAS_LINE//\*/\\*}/d" "$SHELL_RC"
                sed -i "/${ALIAS_COMMAND//\*/\\*}/d" "$SHELL_RC"
                log "Removed alias from $SHELL_RC"
            fi
        else
            warn "Alias not found in $SHELL_RC"
        fi
    fi

    # Note: We don't remove the wrapper binary as it might be in use
    log "Uninstall complete. Restart your shell or run: source $SHELL_RC"
    log "Note: ~/.cargo/bin/cargo-wrapper was left in place. Remove manually if desired."
}

install() {
    log "Installing cargo-wrapper..."

    # Create backup directory
    if [[ "$DRY_RUN" != "true" ]]; then
        mkdir -p "$BACKUP_DIR"
        log "Created backup directory: $BACKUP_DIR"
    fi

    # 1. Copy wrapper to ~/.cargo/bin/
    TARGET_DIR="${HOME}/.cargo/bin"
    TARGET_PATH="${TARGET_DIR}/cargo-wrapper"

    if [[ -f "$TARGET_PATH" ]]; then
        if [[ "$DRY_RUN" == "true" ]]; then
            log "[DRY-RUN] Would backup existing $TARGET_PATH to $BACKUP_DIR/"
        else
            cp "$TARGET_PATH" "$BACKUP_DIR/" 2>/dev/null || true
            log "Backed up existing $TARGET_PATH"
        fi
    fi

    if [[ "$DRY_RUN" == "true" ]]; then
        log "[DRY-RUN] Would copy $WRAPPER_SOURCE to $TARGET_PATH"
    else
        mkdir -p "$TARGET_DIR"
        cp "$WRAPPER_SOURCE" "$TARGET_PATH"
        chmod +x "$TARGET_PATH"
        log "Installed cargo-wrapper to $TARGET_PATH"
    fi

    # 2. Ensure ~/.cargo/bin is in PATH (add to shell rc if not)
    PATH_LINE='export PATH="$HOME/.cargo/bin:$PATH"'
    if [[ ":$PATH:" != *":$HOME/.cargo/bin:"* ]]; then
        if ! grep -q 'cargo/bin' "$SHELL_RC" 2>/dev/null; then
            if [[ "$DRY_RUN" == "true" ]]; then
                log "[DRY-RUN] Would add PATH export to $SHELL_RC"
            else
                echo "" >> "$SHELL_RC"
                echo "$PATH_LINE" >> "$SHELL_RC"
                log "Added PATH export to $SHELL_RC"
            fi
        fi
    fi

    # 3. Add alias to shell rc (remove old first, then add new)
    if [[ "$DRY_RUN" == "true" ]]; then
        log "[DRY-RUN] Would add alias to $SHELL_RC:"
        log "[DRY-RUN]   $ALIAS_LINE"
        log "[DRY-RUN]   $ALIAS_COMMAND"
    else
        # Remove any existing alias lines (clean slate)
        sed -i "/${ALIAS_LINE//\*/\\*}/d" "$SHELL_RC" 2>/dev/null || true
        sed -i "/alias cargo=.*cargo-wrapper/d" "$SHELL_RC" 2>/dev/null || true

        # Add new alias block
        {
            echo ""
            echo "$ALIAS_LINE"
            echo "$ALIAS_COMMAND"
        } >> "$SHELL_RC"
        log "Added alias to $SHELL_RC"
    fi

    log "Installation complete!"
    log ""
    log "Next steps:"
    log "  1. Restart your terminal OR run: source $SHELL_RC"
    log "  2. Verify: cargo-wrapper --wrapper-status"
    log "  3. Test: cargo --version (should route through gitforge-build)"
    log ""
    log "To bypass enforcement temporarily: cargo-wrapper --wrapper-fallback -- <cmd>"
}

# Main
if [[ "$UNINSTALL" == "true" ]]; then
    uninstall
else
    install
fi
