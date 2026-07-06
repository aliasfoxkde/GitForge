#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Install Dark Factory git hooks
# Run: ./scripts/install-hooks.sh
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
HOOKS_DIR="$REPO_ROOT/.githooks"
SCRIPTS_HOOKS="$REPO_ROOT/scripts/hooks"

# Create .githooks directory
mkdir -p "$REPO_ROOT/.githooks"

# Copy hooks
cp "$SCRIPTS_HOOKS/pre-commit" "$REPO_ROOT/.githooks/pre-commit"
cp "$SCRIPTS_HOOKS/pre-push" "$REPO_ROOT/.githooks/pre-push"
chmod +x "$REPO_ROOT/.githooks/pre-commit"
chmod +x "$REPO_ROOT/.githooks/pre-push"

# Configure git to use .githooks as hooks path
git config core.hooksPath "$REPO_ROOT/.githooks"

echo "✅ Dark Factory git hooks installed at: $REPO_ROOT/.githooks"
echo "   Run 'git commit' to trigger pre-commit checks"
echo "   Run 'git push' to trigger pre-push checks"
