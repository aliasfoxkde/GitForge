#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Language Detection Script
# Detects project languages and sets GitHub Actions outputs
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

# Detect Go
if [ -f "go.mod" ]; then
    echo "has_go=true" >> "$GITHUB_OUTPUT"
    echo "Go module detected"
else
    echo "has_go=false" >> "$GITHUB_OUTPUT"
fi

# Detect Python
if [ -f "pyproject.toml" ]; then
    # Verify it's a real Python project (has .py files outside template-parts/)
    PY_FILES=$(find . -name "*.py" \
        -not -path "./template-parts/*" \
        -not -path "./.github/*" \
        -not -path "./docs/*" \
        -not -path "./.git/*" \
        -not -path "./venv/*" \
        -not -path "./.venv/*" \
        -not -path "./__pycache__/*" \
        | head -1)
    if [ -n "$PY_FILES" ]; then
        echo "has_python=true" >> "$GITHUB_OUTPUT"
        echo "Python project detected"
    else
        echo "has_python=false" >> "$GITHUB_OUTPUT"
        echo "pyproject.toml found but no Python source files — skipping Python"
    fi
else
    echo "has_python=false" >> "$GITHUB_OUTPUT"
fi

# Detect Rust
if [ -f "Cargo.toml" ]; then
    echo "has_rust=true" >> "$GITHUB_OUTPUT"
    echo "Rust project detected"
else
    echo "has_rust=false" >> "$GITHUB_OUTPUT"
fi

# Detect TypeScript/Node.js
if [ -f "package.json" ]; then
    # Verify it's a real TS project (has .ts/.tsx files)
    TS_FILES=$(find . -name "*.ts" -o -name "*.tsx" \
        -not -path "./template-parts/*" \
        -not -path "./node_modules/*" \
        -not -path "./.github/*" \
        -not -path "./docs/*" \
        -not -path "./.git/*" \
        | grep -v "node_modules" | head -1)
    if [ -n "$TS_FILES" ]; then
        echo "has_typescript=true" >> "$GITHUB_OUTPUT"
        echo "TypeScript project detected"
    else
        echo "has_typescript=false" >> "$GITHUB_OUTPUT"
        echo "package.json found but no TypeScript source files — skipping TypeScript"
    fi
else
    echo "has_typescript=false" >> "$GITHUB_OUTPUT"
fi

# Detect Bash/Shell (has .sh files that are not in .github/workflows or hooks)
if [ -f ".shellcheckrc" ] || find . -name "*.sh" \
    -not -path "./.github/workflows/*" \
    -not -path "./.git/*" \
    -not -path "./template-parts/*" \
    -not -path "./node_modules/*" \
    -not -path "./.githooks/*" \
    | head -1 | grep -q .; then
    echo "has_bash=true" >> "$GITHUB_OUTPUT"
    echo "Bash/Shell project detected"
else
    echo "has_bash=false" >> "$GITHUB_OUTPUT"
fi

echo "---"
echo "Detection complete"
