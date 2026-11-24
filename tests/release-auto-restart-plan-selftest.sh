#!/usr/bin/env bash
# Regression test: a unit masked on this host must be excluded from the
# release-auto restart set.
#
# Why this needs a test (2026-10-10 incident): the NAS runner was retired
# by masking its unit when CI moved to the fedora host. The pre-fix script
# restarted a hardcoded unit list, so every promote tried to restart the
# masked unit, failed, and rolled back - forever. The fix skips masked
# units; without a test, a future edit that restores the hardcoded list
# fails only in production, on the release channel.
#
# The script classifies units by asking systemd, so the test puts a fake
# `systemctl` on PATH and asserts both directions: a masked unit is
# skipped, and a unit that is not masked is still restarted.
#
# The fake answers the query the script actually makes -
# `systemctl show <unit> -p LoadState --value` - which reports "masked"
# for a template instance as well as for a concrete unit file. The older
# `systemctl list-unit-files <unit> --no-legend` form cannot see an
# instance at all, which is how a masked instance reached the restart set
# in the first place.
#
# Usage: bash tests/release-auto-restart-plan-selftest.sh
set -u -o pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
script="$repo_root/scripts/gitforge-release-auto"

[[ -f "$script" ]] || { printf 'selftest: missing %s\n' "$script" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'find "$tmp" -depth -delete 2>/dev/null || true' EXIT

bin="$tmp/bin"
mkdir -p "$bin" "$tmp/repo"

# The fake systemctl reports the units named in FAKE_MASKED as masked and
# everything else as enabled.
cat >"$bin/systemctl" <<'FAKE'
#!/usr/bin/env bash
set -u
# `systemctl show <unit> -p LoadState --value`: the unit is the second
# argument, and the answer is the bare state.
unit="${2:-}"
case ",${FAKE_MASKED:-}," in
    *",$unit,"*) printf 'masked\n' ;;
    *) printf 'enabled\n' ;;
esac
FAKE
chmod +x "$bin/systemctl"

run_plan() {
    PATH="$bin:$PATH" \
    FAKE_MASKED="$1" \
    GITFORGE_AUTO_REPO="$tmp/repo" \
        bash "$script" --print-restart-plan 2>&1
}

fail=0
check_contains() {
    if [[ "$1" == *"$2"* ]]; then
        printf 'ok   %s\n' "$3"
    else
        printf 'FAIL %s: expected to find: %s\n' "$3" "$2" >&2
        printf '%s\n' "$1" >&2
        fail=1
    fi
}
check_absent() {
    if [[ "$1" == *"$2"* ]]; then
        printf 'FAIL %s: did not expect to find: %s\n' "$3" "$2" >&2
        printf '%s\n' "$1" >&2
        fail=1
    else
        printf 'ok   %s\n' "$3"
    fi
}

# - Case 1: a masked runner unit is skipped ---------------------------------
out="$(run_plan "gitforge@runner.service")"
check_contains "$out" "skipping gitforge@runner.service: masked on this host" \
    "masked unit must be reported as skipped"
check_contains "$out" "stateless=gitforge@api.service gitforge@git-server.service" \
    "unmasked stateless units must stay in the plan"
check_contains "$out" "scheduled=gitforge@ci.service" \
    "unmasked scheduled units must stay in the plan"
check_absent "$out" "scheduled=gitforge@runner.service" \
    "masked unit must not appear in the restart plan"

# - Case 2: with nothing masked, every unit is restarted ---------------------
out="$(run_plan "")"
check_absent "$out" "masked on this host" \
    "no unit is masked, so nothing may be reported as skipped"
check_contains "$out" "stateless=gitforge@api.service gitforge@git-server.service" \
    "stateless units must stay in the plan"
check_contains "$out" "scheduled=gitforge@ci.service gitforge@runner.service" \
    "scheduled units must stay in the plan"

# - Case 3: a masked stateless unit is skipped too ---------------------------
out="$(run_plan "gitforge@git-server.service")"
check_contains "$out" "skipping gitforge@git-server.service: masked on this host" \
    "masked stateless unit must be reported as skipped"
check_contains "$out" "stateless=gitforge@api.service" \
    "remaining stateless units must stay in the plan"
check_absent "$out" "stateless=gitforge@api.service gitforge@git-server.service" \
    "masked stateless unit must not appear in the restart plan"

if ((fail)); then
    printf 'selftest: FAILED\n' >&2
    exit 1
fi
printf 'selftest: OK\n'
