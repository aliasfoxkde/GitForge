#!/usr/bin/env bash
# Stop only GitForge services belonging to one checkout.

set -u

ROOT="${GITFORGE_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
DATA_DIR="$ROOT/data"
TERM_WAIT_SECONDS="${GITFORGE_TERM_WAIT_SECONDS:-10}"

case "$TERM_WAIT_SECONDS" in
    ''|*[!0-9]*)
        echo "GITFORGE_TERM_WAIT_SECONDS must be a non-negative integer" >&2
        exit 2
        ;;
esac

is_owned_pid() {
    local pid="$1" expected="$2"
    [ -r "/proc/$pid/cmdline" ] || return 1
    [ "$(tr '\0' ' ' < "/proc/$pid/cmdline" | sed 's/ $//')" = "$expected" ]
}

stop_service() {
    local name="$1" pid deadline
    local executable="$ROOT/target/release/$name"
    while read -r pid; do
        [ -n "$pid" ] || continue
        is_owned_pid "$pid" "$executable" || continue
        echo "Stopping $name (pid $pid)"
        kill -TERM "$pid" 2>/dev/null || true
        deadline=$((SECONDS + TERM_WAIT_SECONDS))
        while is_owned_pid "$pid" "$executable" && [ "$SECONDS" -lt "$deadline" ]; do
            sleep 1
        done
        if is_owned_pid "$pid" "$executable"; then
            echo "$name did not stop after ${TERM_WAIT_SECONDS}s; sending KILL" >&2
            kill -KILL "$pid" 2>/dev/null || true
        fi
    done < <(pgrep -f -x "$executable" || true)
}

for service in api ci git-server runner; do
    stop_service "$service"
done

# PID files are metadata, not authority. Remove only exact files beneath this
# checkout after the process ownership checks above.
for pid_file in "$DATA_DIR/api.pid" "$DATA_DIR/ci.pid" "$DATA_DIR/git.pid" "$DATA_DIR/git-server.pid" "$DATA_DIR/runner.pid"; do
    [ -f "$pid_file" ] && rm -f -- "$pid_file"
done
