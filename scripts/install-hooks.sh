#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Install Dark Factory git hooks
# Run: ./scripts/install-hooks.sh
#
# Installs the .githooks/ directory as the git hooks path.
# Hooks are organized as:
#   .githooks/
#   ├── pre-commit              # Dispatcher — runs quality gates + language hooks
#   ├── pre-commit.d/           # Language-specific pre-commit scripts
#   ├── pre-push               # Dispatcher — runs language-specific push hooks
#   ├── pre-push.d/            # Language-specific pre-push scripts
#   └── lib/                   # Shared utilities
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
HOOKS_DIR="$REPO_ROOT/.githooks"

# Ensure hooks directory exists
if [ ! -d "$HOOKS_DIR" ]; then
    echo "ERROR: .githooks directory not found at $HOOKS_DIR"
    exit 1
fi

# Ensure main hooks are executable
chmod +x "$HOOKS_DIR/pre-commit" "$HOOKS_DIR/pre-push" 2>/dev/null || true

# Ensure sub-hook scripts are executable
chmod +x "$HOOKS_DIR/pre-commit.d/"*.sh 2>/dev/null || true
chmod +x "$HOOKS_DIR/pre-push.d/"*.sh 2>/dev/null || true
chmod +x "$HOOKS_DIR/lib/"*.sh 2>/dev/null || true

# Configure git to use .githooks as hooks path
git config core.hooksPath "$HOOKS_DIR"

echo "✅ Dark Factory git hooks installed from: $HOOKS_DIR"
echo ""
echo "   pre-commit   → quality gates + language-specific checks"
echo "   pre-push    → full test suites per language"
echo ""
echo "   Hooks run automatically on git commit/push."
echo "   Set DEBUG_HOOKS=1 for verbose output."
