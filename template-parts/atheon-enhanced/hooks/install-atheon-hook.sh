#!/bin/bash
# Install Atheon-Enhanced pre-commit hook
#
# Usage: ./hooks/install-atheon-hook.sh
#
# This script:
# 1. Makes the pre-commit hook executable
# 2. Symlinks it to .git/hooks/pre-commit
# 3. Installs atheon if not present

set -e

HOOK_SOURCE="$(dirname "$0")/pre-commit"
GIT_HOOKS_DIR="$(git rev-parse --show-toplevel)/.git/hooks"
TARGET_HOOK="$GIT_HOOKS_DIR/pre-commit"

echo "Installing Atheon-Enhanced pre-commit hook..."

# Make hook executable
chmod +x "$HOOK_SOURCE"
echo "✓ Made hook executable"

# Create hooks directory if it doesn't exist
mkdir -p "$GIT_HOOKS_DIR"

# Backup existing hook if any
if [ -f "$TARGET_HOOK" ] || [ -L "$TARGET_HOOK" ]; then
    BACKUP="$TARGET_HOOK.backup.$(date +%Y%m%d%H%M%S)"
    echo "Backing up existing hook to $BACKUP"
    mv "$TARGET_HOOK" "$BACKUP"
fi

# Symlink the hook
ln -s "$(realpath --relative-to="$GIT_HOOKS_DIR" "$HOOK_SOURCE")" "$TARGET_HOOK"
echo "✓ Symlinked pre-commit hook"

# Check if atheon is installed
if ! command -v atheon &> /dev/null; then
    echo ""
    echo -e "${YELLOW}Warning: atheon not found in PATH${NC}"
    echo ""
    echo "To install Atheon-Enhanced:"
    echo "  Linux/macOS: curl -sSL https://raw.githubusercontent.com/aliasfoxkde/Atheon-Enhanced/main/scripts/install.sh | bash"
    echo "  Or download: https://github.com/aliasfoxkde/Atheon-Enhanced/releases"
    echo ""
    echo "Set \$ATHEON_BINARY to custom path if needed"
fi

echo ""
echo -e "${GREEN}✓ Atheon-Enhanced pre-commit hook installed${NC}"
echo ""
echo "The hook will scan staged files for secrets and PII before each commit."
echo "To bypass: git commit --no-verify"
