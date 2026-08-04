#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Bash Script Template
# Features: strict mode, error handling, logging, argument parsing, safe defaults
# ─────────────────────────────────────────────────────────────────────────────

set -Eeuo pipefail  # STRICT MODE: all required

# ─── Script Identity ───────────────────────────────────────────────────────────
readonly SCRIPT_NAME="$(basename "$0")"
readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_VERSION="1.0.0"

# ─── Colors ───────────────────────────────────────────────────────────────────
readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly YELLOW='\033[1;33m'
readonly BLUE='\033[0;34m'
readonly NC='\033[0m' # No Color

# ─── Logging ───────────────────────────────────────────────────────────────────
log_info()  { echo -e "${GREEN}[INFO]${NC} $*"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC} $*" >&2; }
log_error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }
log_debug() { [[ "${DEBUG:-0}" == "1" ]] && echo -e "${BLUE}[DEBUG]${NC} $*" || true; }

# ─── Exit Handlers ─────────────────────────────────────────────────────────────
# shellcheck disable=SC2317
cleanup_on_exit() {
    local exit_code=$?
    # Add cleanup logic here
    exit $exit_code
}
trap cleanup_on_exit EXIT

# ─── Argument Parsing ──────────────────────────────────────────────────────────
usage() {
    cat <<EOF
Usage: $SCRIPT_NAME [OPTIONS] <arg>

$SCRIPT_NAME does X.

Options:
    -h, --help         Show this help message
    -v, --version      Show version
    -d, --debug        Enable debug mode
    -q, --quiet        Suppress output
    -t, --timeout N    Timeout in seconds (default: 300)

Examples:
    $SCRIPT_NAME -t 60 myfile.txt
    DEBUG=1 $SCRIPT_NAME myfile.txt
EOF
}

# Parse arguments
QUIET=0
DEBUG=0
TIMEOUT=300

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help) usage; exit 0 ;;
        -v|--version) echo "$SCRIPT_VERSION"; exit 0 ;;
        -d|--debug) DEBUG=1; shift ;;
        -q|--quiet) QUIET=1; shift ;;
        -t|--timeout) TIMEOUT="$2"; shift 2 ;;
        -*) log_error "Unknown option: $1"; usage; exit 1 ;;
        *)  break ;;
    esac
done

[[ $# -lt 1 ]] && { usage; exit 1; }

ARG="${1:-}"

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    log_info "Starting $SCRIPT_NAME (version $SCRIPT_VERSION)"

    # TODO: Add your logic here

    log_info "Done"
}

main "$@"
