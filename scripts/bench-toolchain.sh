#!/usr/bin/env bash
# bench-toolchain.sh — run one benchmark matrix cell (issue #341, M7).
# Pure measurement: captures environment ONCE at start into variables and
# never re-reads it afterward. No decision logic.
#
# Usage:
#   GIT_SHA=<sha> TARGET_DIR=<dir> TMPDIR=<dir> OUT_DIR=<dir> \
#     bench-toolchain.sh <cell-id> -- <command...>
#
# Emits one JSON object on stdout and to $OUT_DIR/<cell-id>.json.
set -u

# --- capture environment at the edge, once ---
CELL_ID="${1:?usage: bench-toolchain.sh <cell-id> -- <command...>}"
[ "${2:-}" = "--" ] || { echo "expected -- after cell id" >&2; exit 64; }
shift 2

HOSTNAME_NOW="$(hostname -s)"
GIT_SHA="${GIT_SHA:-unknown}"
TARGET_DIR="${TARGET_DIR:-/var/tmp/gitforge-target}"
TMPDIR_NOW="${TMPDIR:-/tmp}"
OUT_DIR="${OUT_DIR:-docs/planning/m7-raw}"
TIMESTAMP_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"   # taken at the edge
LOAD_AVG="$(cut -d' ' -f1-3 /proc/loadavg)"

CMD_STR="$(printf '%q ' "$@")"

# --- timed section: only variables captured above are used ---
START_NS="$(date +%s.%N)"
timeout 900 env CARGO_TARGET_DIR="$TARGET_DIR" TMPDIR="$TMPDIR_NOW" "$@"
EXIT_CODE=$?
END_NS="$(date +%s.%N)"

WALL="$(awk -v s="$START_NS" -v e="$END_NS" 'BEGIN{printf "%.2f", e-s}')"

JSON_FMT='{"cell_id":"%s","command":"%s","wall_seconds":%s,"exit_code":%d,"host":"%s","timestamp_utc":"%s","git_sha":"%s","target_dir":"%s","load_avg":"%s"}\n'
json="$(printf "$JSON_FMT" \
  "$CELL_ID" "$CMD_STR" "$WALL" "$EXIT_CODE" \
  "$HOSTNAME_NOW" "$TIMESTAMP_UTC" "$GIT_SHA" "$TARGET_DIR" "$LOAD_AVG")"

mkdir -p "$OUT_DIR"
printf '%s\n' "$json" | tee "$OUT_DIR/$CELL_ID.json"
exit $EXIT_CODE
