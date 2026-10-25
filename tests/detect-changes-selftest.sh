#!/usr/bin/env bash
# detect-changes-selftest.sh — run scripts/detect-changes.sh against
# synthetic repositories and assert the CI_CHANGE_* classification.
#
# Covers the failure classes observed in production:
#   - branch push (merge-base diff is the change set)
#   - push to main (merge-base == HEAD; the tip-diff fallback must fire)
#   - docs-only change must not set INCLUDE_CODE
#   - Rust change must set INCLUDE_CODE and INCLUDE_CRATES
#
# Usage: bash tests/detect-changes-selftest.sh
# Exits non-zero if any assertion fails. Requires only git + bash.
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DETECT="$SCRIPT_DIR/../scripts/detect-changes.sh"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

pass=0
fail=0

run_detect() {
    (cd "$1" && bash "$DETECT")
}

get_var() {
    grep "^$2=" <<< "$1" | cut -d= -f2
}

assert_eq() {
    if [[ "$2" == "$3" ]]; then
        pass=$((pass + 1))
    else
        echo "FAIL $1: expected [$2] got [$3]"
        fail=$((fail + 1))
    fi
}

gitc() { command git -c user.email=t@t -c user.name=t "$@"; }

### Case 1: branch push — merge-base diff is the change set
R="$WORK/branch"
gitc init -q -b main "$R"
(
    cd "$R"
    echo docs > README.md
    gitc add .
    gitc commit -qm base
    gitc checkout -qb feat
    mkdir -p crates/gitforge-x/src
    echo "fn main(){}" > crates/gitforge-x/src/lib.rs
    echo amended >> README.md
    gitc add .
    gitc commit -qm code
    gitc init -q --bare "$R-origin.git"
    gitc remote add origin "$R-origin.git"
    gitc push -q origin main
    gitc fetch -q origin main
)
OUT=$(run_detect "$R")
assert_eq "branch: file count" "2" "$(get_var "$OUT" CI_CHANGE_FILE_COUNT)"
assert_eq "branch: code" "1" "$(get_var "$OUT" CI_CHANGE_INCLUDE_CODE)"
assert_eq "branch: crates" "1" "$(get_var "$OUT" CI_CHANGE_INCLUDE_CRATES)"
assert_eq "branch: docs" "1" "$(get_var "$OUT" CI_CHANGE_INCLUDE_DOCS)"

### Case 2: push to main — merge-base == HEAD, tip-diff fallback must fire
R2="$WORK/mainpush"
gitc init -q -b main "$R2"
(
    cd "$R2"
    echo a > README.md
    gitc add .
    gitc commit -qm base
    gitc init -q --bare "$R2-origin.git"
    gitc remote add origin "$R2-origin.git"
    gitc push -q origin main
    echo "fn main(){}" > src-main.rs
    gitc add .
    gitc commit -qm tip
    gitc fetch -q origin main
)
OUT2=$(run_detect "$R2")
assert_eq "mainpush: file count" "1" "$(get_var "$OUT2" CI_CHANGE_FILE_COUNT)"
assert_eq "mainpush: code" "1" "$(get_var "$OUT2" CI_CHANGE_INCLUDE_CODE)"

### Case 3: docs-only change must not trip the code class
R3="$WORK/docs"
gitc init -q -b main "$R3"
(
    cd "$R3"
    echo a > README.md
    gitc add .
    gitc commit -qm base
    echo b >> README.md
    gitc add .
    gitc commit -qm docs-only
    gitc init -q --bare "$R3-origin.git"
    gitc remote add origin "$R3-origin.git"
    gitc push -q origin main
    gitc fetch -q origin main
)
OUT3=$(run_detect "$R3")
assert_eq "docs: code" "0" "$(get_var "$OUT3" CI_CHANGE_INCLUDE_CODE)"
assert_eq "docs: docs" "1" "$(get_var "$OUT3" CI_CHANGE_INCLUDE_DOCS)"

echo "detect-changes selftest: $pass passed, $fail failed"
((fail == 0))
