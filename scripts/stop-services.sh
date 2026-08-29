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

owned_pids() {
    local executable="$1" proc pid resolved
    for proc in /proc/[0-9]*; do
        pid="${proc##*/}"
        [ -r "$proc/exe" ] || continue
        resolved="$(readlink -f -- "$proc/exe" 2>/dev/null || true)"
        [ "$resolved" = "$executable" ] && printf '%s\n' "$pid"
    done
}

stop_service() {
    local name="$1" pid deadline
    local executable="$ROOT/target/release/$name"
    while read -r pid; do
        [ -n "$pid" ] || continue
        [ -r "/proc/$pid/exe" ] || continue
        echo "Stopping $name (pid $pid)"
        kill -TERM "$pid" 2>/dev/null || true
        deadline=$((SECONDS + TERM_WAIT_SECONDS))
        while [ "$(readlink -f -- "/proc/$pid/exe" 2>/dev/null || true)" = "$executable" ] && [ "$SECONDS" -lt "$deadline" ]; do
            sleep 1
        done
        if [ "$(readlink -f -- "/proc/$pid/exe" 2>/dev/null || true)" = "$executable" ]; then
            echo "$name did not stop after ${TERM_WAIT_SECONDS}s; sending KILL" >&2
            kill -KILL "$pid" 2>/dev/null || true
        fi
    done < <(owned_pids "$executable")
}

for service in api ci git-server runner; do
    stop_service "$service"
done

# PID files are metadata, not authority. Remove only exact files beneath this
# checkout after the process ownership checks above.
for pid_file in "$DATA_DIR/api.pid" "$DATA_DIR/ci.pid" "$DATA_DIR/git.pid" "$DATA_DIR/git-server.pid" "$DATA_DIR/runner.pid"; do
    if [ -f "$pid_file" ]; then
        rm -f -- "$pid_file"
    fi
done
