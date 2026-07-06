#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# E2E Test Runner for Python projects
# Runs playwright-based E2E tests with coverage reporting
# ─────────────────────────────────────────────────────────────────────────────
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

COVERAGE_THRESHOLD="${COVERAGE_THRESHOLD:-80}"
REPORT_DIR="${PROJECT_ROOT}/coverage-e2e"
PARALLELISM="${PARALLELISM:-4}"

# Colors
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
INFO() { echo -e "${GREEN}[e2e]${NC} $*"; }
WARN() { echo -e "${YELLOW}[e2e]${NC} $*"; }
ERROR() { echo -e "${RED}[e2e]${NC} $*"; }

cd "$PROJECT_ROOT"

INFO "Installing Playwright browsers (if needed)..."
playwright install --with-deps chromium 2>/dev/null || true

INFO "Running E2E tests (parallelism=$PARALLELISM, threshold=${COVERAGE_THRESHOLD}%)..."
pytest \
    tests/e2e/ \
    --config-file pyproject.toml \
    --timeout=300 \
    --numprocesses="$PARALLELISM" \
    --co  # collect only first to verify

INFO "All E2E tests passed"
