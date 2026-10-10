# Branch Strategy

## Overview

GitForge uses a trunk-based model with short-lived feature branches off
`main`. There is no `develop` line and no long-lived release branches.

```
main (production)
  └── [short-lived feature/fix/docs/test/chore branches]
       ↓
       PR → GitForge pipeline green → merge commit → main
```

### Main Branch

- **Name:** `main`
- **Protection:** GitHub ruleset (Safeguards) + the repo's own GitForge
  pipeline as the authoritative gate (see below)
- **State:** Always production-ready; a green run on the exact HEAD
  commit is the standing invariant
- **History:** Preserved via merge commits — never squash

## The Gate Is GitForge, Not GitHub

The authoritative CI chain is this repo's self-hosted pipeline
(`.gitforge.yml`:
`fmt → clippy (+ rustdoc gate) → test → coverage` on `dsc-ci-rust:8`,
linear chain, one shared workspace per run). A push to the GitForge
remote triggers it automatically.

GitHub Actions workflows mirror the checks for the public record, but a
red GitHub run is **not** a code-failure signal on this instance (the
GitHub account is billing-blocked; NAS-contended runners). Validate
through the GitForge pipeline before merging.

Before pushing, mirror the chain locally:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=2   # the 2-thread bound is deliberate
```

## Branch Types

| Prefix | Purpose | Example |
|--------|---------|---------|
| `feat/` / `feature/` | New functionality | `feat/release-gate` |
| `fix/` | Bug fixes | `fix/repo-create-owner-prefix` |
| `docs/` | Documentation only | `docs-v0612-release-record` |
| `test/` | Test improvements | `test/api-route-coverage-20261002` |
| `chore/` | Maintenance, supply chain | `chore/close-window-rehearsal` |
| `refactor/`, `perf/`, `ci/` | Code restructuring / performance / pipeline work | `refactor/scanner-interface` |
| `<campaign>-*` | Multi-commit campaign branches | `r6-platform-durability` |

## Workflow

### Creating a Branch

```bash
git checkout main && git pull
git checkout -b fix/my-fix
# ... work ...
git push -u gitforge-ci fix/my-fix   # GitForge first — this triggers the pipeline
git push -u origin fix/my-fix        # then the GitHub mirror
```

### Merging

1. GitForge pipeline green on the branch (all four jobs, including the
   coverage gate — fail < 82%, warn < 84% lines, calibrated in-sandbox).
2. Open the PR against `main`.
3. Merge with a **merge commit** (`gh pr merge --merge --admin`).
   `main`'s ruleset requires the temporary-bypass procedure: back up
   `bypass_actors` via GET, PUT the bypass in, merge, PUT restore to
   `[]` immediately — see `docs/MIRROR_POLICY.md`. The bypass window
   is two API calls, never a standing state.
4. If the change is release-bound, a green run of the **merge commit
   itself** is required before cutting (the release gate checks the
   exact SHA).

### Hotfix Process

Same flow, expedited: branch from `main`, fix with a pinned regression
test, validate through the pipeline, merge, tag, and release. Tag
pushes follow the release sequence in `docs/RUNBOOK.md`.

## Releases

Tags are cut on `main` only after the exact-SHA pipeline run is green.
The release sequence is: `scripts/gitforge-release-gate` (refuses a cut
without a green run of the exact source commit) →
`scripts/gitforge-release-bundle` → `gitforge-release-promote --apply`
→ drain-gated restart of the `gitforge@*` units → GitHub tag + release
pushed GitForge-first, then mirrored. See `docs/RUNBOOK.md` for the
authoritative steps and `docs/MIRROR_POLICY.md` for what belongs where.

## Best Practices

- **Short-lived branches** — land within a day or two; rebase onto
  `main` rather than letting the branch drift
- **Atomic commits** — one logical change per commit
- **Conventional commits** — `feat:`, `fix:`, `docs:`, `test:`,
  `refactor:`, `chore:`, `ci:`
- **Pinned regression tests** — every fixed defect gets a test that
  fails without the fix
- **No force-pushes** to `main` (`--force-with-lease` only, and only
  for your own just-pushed branch)
- **Delete merged branches** — after verifying the merge actually
  landed on `main` (ancestry check, not branch absence)
