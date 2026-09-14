# GitForge reconciler P0-5a worker handoff

**Repository/worktree:** `/nas/Temp/work/gitforge-container-reconciler-20260914`  
**Branch:** `codex/container-reconciler-20260914`  
**Base:** `origin/main` at `235292df`  
**Execution:** Fedora `192.168.0.201`, via the durable worker dispatcher

## One objective

Replace the candidate reconciler’s unsafe use of Docker `Created` as `exited_at_ms` with authoritative container finish-time acquisition. A stopped container with an old creation time but a recent finish time must remain inside the grace period. Missing, malformed, or unavailable finish time must retain the container.

## Constraints

- Work only in the named isolated worktree and the reconciler-related files/tests.
- Do not touch the canonical GitForge checkout, delete containers, merge, push, or change deletion defaults.
- Use the existing Bollard version and GitForge error/result conventions.
- Make the first source/test edit promptly; do not spend the task producing a prose plan.
- Preserve the existing fail-closed ownership, active-job, running-state, and policy behavior.

## Required implementation

1. Inspect the current `DockerContainerSource` and Bollard models to determine the authoritative `FinishedAt` field and its timestamp format.
2. Add the minimum source abstraction needed to inspect each managed container and populate `ContainerRecord.exited_at_ms` from `FinishedAt`.
3. Keep unknown/invalid finish times as `None`; never fall back to `Created` for grace eligibility.
4. Add focused tests for: old `Created` + recent `FinishedAt` retained; old `FinishedAt` eligible; missing/invalid finish retained; running retained.
5. Run `cargo fmt --all`, `cargo check -p gitforge-sandbox -p gitforge-runner`, and focused reconciler tests with a hard outer timeout.

## Completion evidence

Leave changed files in the worktree and return a concise summary containing the exact diff scope, commands/results, and unresolved blockers. A green test without the timestamp semantic test is insufficient. Do not claim production acceptance; P0-5b/5c/5d and full GitForge gates remain separate.
