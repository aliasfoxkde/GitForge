# ADR: CI Boot Ordering — Event Consumer Before Recovery

**Status:** Accepted — 2026-10-10
**Evidence:** commit 69e3c859; observed 2026-10-08 (triggers failing
40+ minutes after a ci restart under load); memory
"ci boot blocks behind workspace rebuild".

## Context

The CI orchestrator's startup sequence inlined `rebuild_live_engines`
before spawning the event consumer. The rebuild re-clones run
workspaces and, under load or with many non-terminal runs, takes many
minutes. Every trigger that arrived during that window was accepted by
the API and durably recorded but had no consumer to publish it into a
pipeline run — post-restart triggers 503'd or sat latent for the whole
rebuild.

Recovery and triggering are independent concerns: reconciliation
grades from durable rows (a run whose rows are all terminal is
finalizable regardless of which engine believes it owns it), and a
startup rebuild racing a fresh trigger contends only on the registry
lock.

## Decision

In `services/ci/src/main.rs`, the event consumer spawn (and the shared
shutdown flag it holds a handle to) precedes the recovery passes —
inline rebuild, redrive pass, reconcile loop. The recovery passes
remain spawned so a large sweep cannot delay startup further.

## Consequences

- Post-restart triggers are consumed as soon as the API can record
  them; the rebuild no longer gates them.
- A trigger arriving mid-rebuild may run concurrently with the rebuild
  for the same repo. This is safe by construction: lanes are per-repo
  (`GITFORGE_TRIGGER_CONCURRENCY`, default 4), engine creation takes
  the registry lock, and the rebuild grafts durable truth — but it is
  the one interaction to watch if workspace-clone contention ever
  shows up in journals.
- The boot sequence invariant (consumer first, recovery spawned) is
  documented here and in the code comment at the spawn site.
