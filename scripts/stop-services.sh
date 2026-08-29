#!/usr/bin/env bash
set -euo pipefail

# Stop only binaries belonging to this checkout. A broad `pkill -f
# target/release/...` can terminate another worktree, a CI job, or an agent.
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"

echo "Stopping GitForge services from ${ROOT}..."

stop_service() {
  local name="$1"
  local binary="${ROOT}/target/release/${name}"
  local stopped=0
  while read -r pid; do
    [[ -n "$pid" ]] || continue
    if kill -0 "$pid" 2>/dev/null; then
      kill -TERM "$pid" 2>/dev/null || true
      echo "${name} stopped (pid ${pid})"
      stopped=1
    fi
  done < <(pgrep -f -x -- "$binary" || true)
  [[ "$stopped" -eq 1 ]] || echo "${name} not running"
}

stop_service api
stop_service ci
stop_service git-server
stop_service runner

if [[ -d "${ROOT}/data" ]]; then
  find "${ROOT}/data" -maxdepth 1 -type f -name '*.pid' -delete
fi

echo "GitForge services stopped"
