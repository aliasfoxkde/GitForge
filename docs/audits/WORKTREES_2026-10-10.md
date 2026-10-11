# Worktree Census & GC Audit — 2026-10-10

Scope: every registered worktree of `/nas/Temp/repos/GitForge` (paths under
`/nas/Temp`, `/home/mkinney`). Multi-agent repo — see the standing rule:
**verify ownership before touching any dirty tree**; dirty trees below are
co-tenant WIP unless marked ours.

## Actions taken

- Pruned 3 dead entries (`git worktree prune`): their working dirs were
  already gone, only the admin metadata remained —
  `gitforge-cpu-limits-20260924`, `gitforge-cpu-quota-nvme-20260927`,
  `gitforge-terminal-job-requeue-20261005` (all under `~/.cache/`).
- Nothing else removed. 44 live worktrees remain.

## Census (after prune)

| State | Count |
|---|---:|
| clean | 25 |
| dirty, ≤ 9 files (co-tenant WIP in progress) | 17 |
| dirty, 22 files (`/nas/Temp/work/gitforge-243`, **on `main`**, staged) | 1 |
| dirty, 3962 files (build-artifact spill), **locked** | 1 |

## Dispositions requiring an owner decision

- `/nas/Temp/work/gitforge-sha-exec-env-20260913`
  (`codex/sha-evidence-exec-env-20260913`, lock: "initializing", one month
  old): the 3962-file dirt is `target/`-class spill, not source edits, and
  the lock predates any live session. GC candidate after the branch's
  commits are confirmed merged — owner: codex.
- `/nas/Temp/work/gitforge-243` (branch `main`, 22 staged files incl.
  `routes/{ci,repo,webhook}.rs`, `openapi.rs`): blocks the local `main`
  fast-forward. Owner: agent gitforge-243. Do not touch; rebase its WIP or
  wait for it to clear.
- Everything else dirty: 1–9 file WIP from named dated branches
  (`codex/*-2026100x`), consistent with active work. Re-audit after
  2026-10-17; trees whose branch date is ≥ 2 weeks old and whose commits
  are ancestry-merged into main are the next prune batch (GitForge
  promotion rule: only ancestry-merged branches are provably deletable).

## Re-running the census

```bash
git worktree prune -v          # metadata-only; dirs must already be gone
git worktree list --porcelain | awk '/^worktree /{print $2}' |
  while read -r w; do
    n=$(git -C "$w" status --porcelain 2>/dev/null | wc -l)
    printf '%s\t%s\t%s\n' "$((n>0?n:0))" "$(git -C "$w" branch --show-current)" "$w"
  done | sort -rn
```
