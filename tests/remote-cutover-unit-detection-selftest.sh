#!/usr/bin/env bash
# Regression selftest for the cutover's unit detection.
#
# The cutover decides which units to stop and start from what systemctl
# reports. A detection gap there is a SILENT NO-OP RELEASE: the symlink
# flips, nothing restarts, and the old binaries keep serving while the
# cutover reports success. That is exactly what happened on the NAS
# profile, where every unit is an INSTANCE of the gitforge@.service
# template and `systemctl list-unit-files` enumerates no instance at all.
#
# The cases below drive the real script with a stubbed systemctl that
# reproduces the two topologies the cutover meets:
#   - the NAS profile: template instances known to the manager, absent from
#     list-unit-files, one of them retired (masked),
#   - the fedora profile: concrete unit files that were never started, so
#     the manager knows nothing about them at runtime.
set -u -o pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
script="$repo_root/scripts/gitforge-remote-cutover"

[[ -f "$script" ]] || { printf 'selftest: missing %s\n' "$script" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'find "$tmp" -depth -delete 2>/dev/null || true' EXIT

mkdir -p "$tmp/bin" "$tmp/bundle/bin"
: >"$tmp/db"
install -m 0755 /dev/null "$tmp/bundle/bin/ci"
ln -sfn "$tmp/bundle" "$tmp/link"

cat >"$tmp/bin/systemctl" <<'STUB'
#!/usr/bin/env bash
# Stub systemctl. Fixture files, one name [state] per line:
#   GF_FIXTURE_UNIT_FILES  concrete unit files (what list-unit-files sees)
#   GF_FIXTURE_UNITS       instances the manager knows (what list-units sees)
#   GF_FIXTURE_MASKED      retired instances (LoadState=masked)
set -u
verb=""
unit=""
for arg in "$@"; do
    case "$arg" in
        --user|--system|--all|--no-legend|--plain) continue ;;
    esac
    if [[ -z "$verb" ]]; then
        case "$arg" in
            list-unit-files|list-units|show) verb="$arg" ;;
        esac
        continue
    fi
    case "$arg" in
        -p|--value|LoadState) continue ;;
    esac
    unit="$arg"
done

case "$verb" in
    list-unit-files)
        [[ -s "${GF_FIXTURE_UNIT_FILES:-}" ]] || exit 0
        while read -r name state; do
            [[ -n "$name" ]] || continue
            [[ "$name" == "$unit" ]] && printf '%s %s\n' "$name" "$state"
        done <"$GF_FIXTURE_UNIT_FILES"
        ;;
    list-units)
        [[ -s "${GF_FIXTURE_UNITS:-}" ]] || exit 0
        while read -r name state; do
            [[ -n "$name" ]] || continue
            [[ "$name" == "$unit" ]] && printf '%s %s active running Stub Unit\n' "$name" "$state"
        done <"$GF_FIXTURE_UNITS"
        ;;
    show)
        if [[ -s "${GF_FIXTURE_MASKED:-}" ]]; then
            while read -r name; do
                [[ -n "$name" ]] || continue
                [[ "$name" == "$unit" ]] && { printf 'masked\n'; exit 0; }
            done <"$GF_FIXTURE_MASKED"
        fi
        if [[ -s "${GF_FIXTURE_UNITS:-}" ]]; then
            while read -r name state; do
                [[ -n "$name" ]] || continue
                [[ "$name" == "$unit" ]] && { printf '%s\n' "$state"; exit 0; }
            done <"$GF_FIXTURE_UNITS"
        fi
        printf 'not-found\n'
        ;;
esac
exit 0
STUB
chmod +x "$tmp/bin/systemctl"

: >"$tmp/unit_files"
: >"$tmp/units"
: >"$tmp/masked"
export GF_FIXTURE_UNIT_FILES="$tmp/unit_files"
export GF_FIXTURE_UNITS="$tmp/units"
export GF_FIXTURE_MASKED="$tmp/masked"

run_cutover() {
    PATH="$tmp/bin:$PATH" bash "$script" \
        --units "$1" --db "$tmp/db" --bundle "$tmp/bundle" --link "$tmp/link" --dry-run 2>&1
}

fail=0
check() {
    if printf '%s\n' "$3" | grep -qF -- "$2"; then
        printf 'ok   %s\n' "$1"
    else
        printf 'FAIL %s: expected to find: %s\n' "$1" "$2" >&2
        printf '%s\n' "$3" >&2
        fail=1
    fi
}
check_absent() {
    if printf '%s\n' "$3" | grep -qF -- "$2"; then
        printf 'FAIL %s: expected NOT to find: %s\n' "$1" "$2" >&2
        printf '%s\n' "$3" >&2
        fail=1
    else
        printf 'ok   %s\n' "$1"
    fi
}

# --- Case A: NAS profile, template instances known to the manager -----------
printf 'gitforge@api.service loaded\ngitforge@ci.service loaded\ngitforge@git-server.service loaded\ngitforge@runner.service loaded\n' >"$tmp/units"
out=$(run_cutover system)
check "template instance schedulers detected" \
    "drain gate: scheduler units gitforge@ci.service gitforge@runner.service" "$out"
check "template instance stateless detected" \
    "stateless units: gitforge@api.service gitforge@git-server.service" "$out"

# --- Case B: one retired instance (masked) ---------------------------------
printf 'gitforge@api.service loaded\ngitforge@ci.service loaded\ngitforge@git-server.service loaded\n' >"$tmp/units"
printf 'gitforge@runner.service\n' >"$tmp/masked"
out=$(run_cutover system)
check "masked instance reported as retired" \
    "skipping gitforge@runner.service: masked (retired on this host)" "$out"
check "masked instance excluded from the scheduler set" \
    "drain gate: scheduler units gitforge@ci.service" "$out"
check_absent "masked instance not restarted" \
    "scheduler units gitforge@ci.service gitforge@runner.service" "$out"

# --- Case C: fedora profile, concrete unit files never started -------------
: >"$tmp/masked"
: >"$tmp/units"
printf 'gitforge-api.service enabled\ngitforge-ci.service enabled\ngitforge-git-server.service enabled\ngitforge-runner.service enabled\n' >"$tmp/unit_files"
out=$(run_cutover user)
check "concrete unit file schedulers detected" \
    "drain gate: scheduler units gitforge-ci gitforge-runner" "$out"
check "concrete unit file stateless detected" \
    "stateless units: gitforge-api gitforge-git-server" "$out"

# --- Case D: nothing deployed on this host ---------------------------------
: >"$tmp/unit_files"
out=$(run_cutover system)
check "no units known means no scheduler restart" \
    "drain gate: scheduler units (none)" "$out"
check "no units known means no stateless restart" \
    "stateless units: (none)" "$out"

if ((fail)); then
    printf 'selftest: FAILED\n' >&2
    exit 1
fi
printf 'selftest: OK\n'
