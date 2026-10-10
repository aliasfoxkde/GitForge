#!/usr/bin/env bash
# Unwrap/expect/panic inventory per crate — the metric behind the
# unwrap-elimination campaign. This is a measuring tool, not a gate:
# counts are recorded in docs/planning/QUALITY_BASELINE.md and ratcheted
# down per crate; the enforcement lints (clippy::unwrap_used et al.)
# attach per crate as each crate's count reaches zero.
#
# Usage: scripts/unwrap-count.sh [crate-dir]...   (default: all crates+services)
set -euo pipefail

roots=("$@")
if ((${#roots[@]} == 0)); then
  mapfile -t roots < <(find crates services -maxdepth 1 -mindepth 1 -type d | sort)
fi

total=0
printf '%-28s %8s %8s %8s\n' crate unwrap expect panic
for root in "${roots[@]}"; do
  # Library and route code only: skip tests/ dirs and #[cfg(test)] modules
  # are unavoidable here (grep is line-based) — the count is a trend
  # metric, and test files legitimately unwrap.
  src="$root/src"
  [ -d "$src" ] || continue
  unwrap=$(grep -R '\.unwrap()' "$src" --include='*.rs' -c 2>/dev/null | awk -F: '{s+=$NF} END {print s+0}' || true)
  expect=$(grep -R '\.expect(' "$src" --include='*.rs' -c 2>/dev/null | awk -F: '{s+=$NF} END {print s+0}' || true)
  panic=$(grep -R 'panic!(' "$src" --include='*.rs' -c 2>/dev/null | awk -F: '{s+=$NF} END {print s+0}' || true)
  printf '%-28s %8d %8d %8d\n' "$(basename "$root")" "$unwrap" "$expect" "$panic"
  total=$((total + unwrap + expect + panic))
done
printf '%-28s %8d\n' TOTAL "$total"
