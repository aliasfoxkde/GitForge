#!/bin/bash
# backup-cargo-wrapper.sh - Backup cargo-wrapper configuration
#
# Usage: ./backup-cargo-wrapper.sh [--restore PATH]
#
# Creates a timestamped backup of:
#   - ~/.cargo/bin/cargo-wrapper
#   - Shell rc files with cargo alias
#   - gitforge-build daemon configuration

set -e

BACKUP_ROOT="${HOME}/.cargo-wrapper-backups"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_DIR="${BACKUP_ROOT}/${TIMESTAMP}"
DRY_RUN=false

usage() {
    cat << EOF
Usage: $(basename "$0") [OPTIONS]

Backup cargo-wrapper configuration.

OPTIONS:
    --dry-run       Show what would be backed up without copying
    --restore PATH  Restore from backup directory (prints restore commands)
    --list          List available backups
    --help          Show this help

EXAMPLES:
    $(basename "$0")                  # Create backup
    $(basename "$0") --dry-run       # Preview backup contents
    $(basename "$0") --list          # Show available backups
    $(basename "$0") --restore ~/.cargo-wrapper-backups/20240101_120000

EOF
    exit 0
}

log() { echo "[backup] $1"; }
warn() { echo "[backup] WARNING: $1" >&2; }

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --restore) RESTORE_PATH="$2"; shift 2 ;;
        --list) LIST_ONLY=true; shift ;;
        --help) usage ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

list_backups() {
    if [[ -d "$BACKUP_ROOT" ]]; then
        echo "Available backups in $BACKUP_ROOT:"
        ls -1td "$BACKUP_ROOT"/*/ 2>/dev/null | head -20
    else
        echo "No backups found in $BACKUP_ROOT"
    fi
}

restore_backup() {
    if [[ -z "$RESTORE_PATH" ]]; then
        echo "Error: --restore requires a path"
        exit 1
    fi

    if [[ ! -d "$RESTORE_PATH" ]]; then
        echo "Error: Backup directory not found: $RESTORE_PATH"
        list_backups
        exit 1
    fi

    echo "Restore instructions for: $RESTORE_PATH"
    echo ""
    echo "Run these commands to restore:"
    echo ""

    # Shell rc
    if [[ -f "${RESTORE_PATH}/bashrc_aliases" ]]; then
        echo "# Restore alias to ~/.bashrc:"
        echo "cat '${RESTORE_PATH}/bashrc_aliases' >> ~/.bashrc"
        echo ""
    fi

    # Wrapper
    if [[ -f "${RESTORE_PATH}/cargo-wrapper" ]]; then
        echo "# Restore cargo-wrapper binary:"
        echo "cp '${RESTORE_PATH}/cargo-wrapper' ~/.cargo/bin/cargo-wrapper"
        echo "chmod +x ~/.cargo/bin/cargo-wrapper"
        echo ""
    fi
}

backup_file() {
    local src="$1"
    local dest="$2"

    if [[ ! -f "$src" ]]; then
        return 1
    fi

    if [[ "$DRY_RUN" == "true" ]]; then
        log "[DRY-RUN] Would copy: $src -> $dest"
    else
        mkdir -p "$(dirname "$dest")"
        cp "$src" "$dest"
        log "Backed up: $src"
    fi
}

do_backup() {
    log "Starting backup to: $BACKUP_DIR"

    if [[ "$DRY_RUN" == "true" ]]; then
        log "[DRY-RUN] Mode - no files will be copied"
    fi

    mkdir -p "$BACKUP_DIR"

    # 1. Backup cargo-wrapper binary
    backup_file "${HOME}/.cargo/bin/cargo-wrapper" "${BACKUP_DIR}/cargo-wrapper"

    # 2. Backup shell rc files (extract cargo alias lines)
    for rcfile in ~/.bashrc ~/.bash_profile ~/.zshrc; do
        if [[ -f "$rcfile" ]]; then
            if grep -q 'cargo-wrapper' "$rcfile" 2>/dev/null; then
                if [[ "$DRY_RUN" == "true" ]]; then
                    log "[DRY-RUN] Would extract alias lines from: $rcfile"
                else
                    grep 'cargo-wrapper' "$rcfile" > "${BACKUP_DIR}/$(basename $rcfile)_aliases"
                    log "Extracted alias from: $rcfile"
                fi
            fi
        fi
    done

    # 3. Backup gitforge-build daemon config
    if [[ -f "/tmp/gitforge-buildd.yaml" ]]; then
        backup_file "/tmp/gitforge-buildd.yaml" "${BACKUP_DIR}/gitforge-buildd.yaml"
    fi

    # 4. Create restore script
    if [[ "$DRY_RUN" != "true" ]]; then
        cat > "${BACKUP_DIR}/restore.sh" << 'RESTORE_EOF'
#!/bin/bash
# Auto-generated restore script
# Run: ./restore.sh

set -e
BACKUP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Restoring from: $BACKUP_DIR"

# Restore wrapper
if [[ -f "$BACKUP_DIR/cargo-wrapper" ]]; then
    mkdir -p ~/.cargo/bin
    cp "$BACKUP_DIR/cargo-wrapper" ~/.cargo/bin/cargo-wrapper
    chmod +x ~/.cargo/bin/cargo-wrapper
    echo "Restored cargo-wrapper"
fi

# Restore aliases
for f in "$BACKUP_DIR"/*_aliases; do
    if [[ -f "$f" ]]; then
        cat "$f" >> ~/.bashrc
        echo "Restored aliases from $f"
    fi
done

echo "Restore complete. Run: source ~/.bashrc"
RESTORE_EOF
        chmod +x "${BACKUP_DIR}/restore.sh"
    fi

    log "Backup complete!"
    echo ""
    echo "Backup location: $BACKUP_DIR"
    echo "To restore: ./restore.sh or $(basename "$0") --restore $BACKUP_DIR"
}

# Main
if [[ "$LIST_ONLY" == "true" ]]; then
    list_backups
elif [[ -n "$RESTORE_PATH" ]]; then
    restore_backup
else
    do_backup
fi
