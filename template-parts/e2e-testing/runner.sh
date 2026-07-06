#!/usr/bin/env bash
# runner.sh — E2E test runner CLI for dark-factory projects.
# Supports browser selection, parallelization, coverage, and multiple report formats.

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────
BROWSER="chromium"
PARALLEL=1
COVERAGE=false
REPORT_FORMAT="list"
TIMEOUT=30000
BASE_URL="${E2E_BASE_URL:-http://localhost:3000}"
REPORT_DIR="${E2E_REPORT_DIR:-./test-reports}"
PLAYWRIGHT_PROJECT=""
ENV_VARS=""

# ── Usage ─────────────────────────────────────────────────────────────────────
usage() {
	cat <<EOF
Usage: $(basename "$0") [OPTIONS]

E2E Test Runner — runs Playwright tests with configurable options.

OPTIONS
  -b, --browser BROWSER     Browser to use: chromium, firefox, webkit (default: chromium)
  -p, --parallel N          Number of parallel workers (default: 1)
  -c, --coverage            Enable coverage collection (default: false)
  -r, --report FORMAT       Report format: list, html, json (default: list)
  -t, --timeout MS          Test timeout in milliseconds (default: 30000)
  -u, --base-url URL        Base URL for tests (default: http://localhost:3000)
  -o, --project NAME        Playwright project name (optional)
  -e, --env KEY=VALUE       Environment variable to pass (can be repeated)
  -h, --help                Show this help message

EXAMPLES
  $(basename "$0") --browser firefox --parallel 4
  $(basename "$0") --coverage --report html --timeout 60000
  $(basename "$0") -b webkit -p 2 -c -r json -e API_KEY=test123

ENVIRONMENT
  E2E_BASE_URL       Base URL (default: http://localhost:3000)
  E2E_REPORT_DIR     Report output directory (default: ./test-reports)
  PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
  PLAYWRIGHT_FIREFOX_EXECUTABLE_PATH
  PLAYWRIGHT_WEBKIT_EXECUTABLE_PATH
EOF
}

# ── Parse Arguments ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
	case "$1" in
		-b|--browser)
			BROWSER="$2"
			shift 2
			;;
		-p|--parallel)
			PARALLEL="$2"
			shift 2
			;;
		-c|--coverage)
			COVERAGE=true
			shift
			;;
		-r|--report)
			REPORT_FORMAT="$2"
			shift 2
			;;
		-t|--timeout)
			TIMEOUT="$2"
			shift 2
			;;
		-u|--base-url)
			BASE_URL="$2"
			shift 2
			;;
		-o|--project)
			PLAYWRIGHT_PROJECT="$2"
			shift 2
			;;
		-e|--env)
			ENV_VARS="${ENV_VARS:+${ENV_VARS} }${2}"
			shift 2
			;;
		-h|--help)
			usage
			exit 0
			;;
		*)
			echo "Unknown option: $1" >&2
			usage >&2
			exit 1
			;;
	esac
done

# ── Validate Arguments ───────────────────────────────────────────────────────
case "$BROWSER" in
	chromium|firefox|webkit) ;;
	*)
		echo "Invalid browser: $BROWSER (must be chromium, firefox, or webkit)" >&2
		exit 1
		;;
esac

if ! [[ "$PARALLEL" =~ ^[0-9]+$ ]] || (( PARALLEL < 1)); then
	echo "Invalid parallel value: $PARALLEL (must be a positive integer)" >&2
	exit 1
fi

if ! [[ "$TIMEOUT" =~ ^[0-9]+$ ]] || (( TIMEOUT < 1000)); then
	echo "Invalid timeout: $TIMEOUT (must be >= 1000ms)" >&2
	exit 1
fi

case "$REPORT_FORMAT" in
	list|html|json) ;;
	*)
		echo "Invalid report format: $REPORT_FORMAT (must be list, html, or json)" >&2
		exit 1
		;;
esac

# ── Build Playwright Arguments ────────────────────────────────────────────────
PW_ARGS=()

# Reporter selection
case "$REPORT_FORMAT" in
	list)   PW_ARGS+=(--reporter=list) ;;
	html)   PW_ARGS+=(--reporter=html) ;;
	json)   PW_ARGS+=(--reporter=json) ;;
esac

# Project filter
if [[ -n "$PLAYWRIGHT_PROJECT" ]]; then
	PW_ARGS+=(--project="$PLAYWRIGHT_PROJECT")
fi

# Parallel workers
PW_ARGS+=(--workers="$PARALLEL")

# Timeout
PW_ARGS+=(--timeout="$TIMEOUT")

# ── Export Environment ────────────────────────────────────────────────────────
export BASE_URL
export E2E_BASE_URL="$BASE_URL"

# Parse and export individual env vars
if [[ -n "$ENV_VARS" ]]; then
	while IFS='=' read -r key value; do
		export "$key"="$value"
	done <<< "$ENV_VARS"
fi

# Coverage flags
if [[ "$COVERAGE" == true ]]; then
	export E2E_COVERAGE=true
	PW_ARGS+=(--coverage)
fi

# ── Ensure Report Directory Exists ─────────────────────────────────────────────
mkdir -p "$REPORT_DIR"

# ── Run Playwright ─────────────────────────────────────────────────────────────
echo "=== E2E Test Runner ==="
echo "Browser:     $BROWSER"
echo "Parallel:    $PARALLEL"
echo "Coverage:    $COVERAGE"
echo "Report:      $REPORT_FORMAT"
echo "Timeout:     ${TIMEOUT}ms"
echo "Base URL:    $BASE_URL"
echo "Report Dir:  $REPORT_DIR"
echo "========================"

# Check if playwright is installed
if ! command -v npx &> /dev/null; then
	echo "Error: npx is required but not installed" >&2
	exit 1
fi

# Run tests
exec npx playwright test "${PW_ARGS[@]}"
