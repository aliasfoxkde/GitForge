#!/usr/bin/env bash
# Validate every first-party frontend template with its committed lockfile.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

for template in "$ROOT/template-parts/vite-react-pwa" "$ROOT/template-parts/vite-ssr"; do
    echo "==> frontend template: ${template#"$ROOT"/}"
    (
        cd "$template"
        pnpm install --frozen-lockfile --ignore-scripts
        pnpm run typecheck
        pnpm run build
        pnpm run lint
        pnpm exec vitest run --maxWorkers=2 --minWorkers=1
        pnpm exec playwright test --workers=1
    )
done
