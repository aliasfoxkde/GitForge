# Services Audit — Git Server and CI Orchestrator

**Date:** 2026-11-21
**Scope:** `services/git-server/`, `services/ci/`
**Finding type:** Research-only audit

---

## 1. Git Server (`services/git-server/src/main.rs`)

### 1.1 Dead Letter Cleanup Loop — Missing Error Propagation in Startup Guard

**File:** `services/git-server/src/main.rs:150-154`

```rust
if state.db_pool.is_some() && state.ci_trigger_url.is_some() && state.ci_trigger_token.is_some() {
    let delivery_state = state.clone();
    tokio::spawn(async move { ci_delivery_loop(delivery_state).await });
    if let Some(pool) = state.dead_letter_pool.clone() {
        tokio::spawn(async move { dead_letter_cleanup_loop(pool).await });
    }
}
```

The `ci_delivery_loop` spawns **regardless** of whether the dead letter pool exists, but `dead_letter_cleanup_loop` only spawns if `dead_letter_pool.is_some()`. Since `dead_letter_pool = saved_db_pool.clone()`, this is equivalent — both require `db_pool`. This is correct but slightly misleading: if `ci_trigger_url` is configured but `DATABASE_URL` is not, `dead_letter_pool` will be `None` and the dead letter cleanup loop silently never runs. The delivery loop will also never run due to the outer guard, so this is not a defect — just an architectural observation.

**Severity:** Low. The guard logic is correct but the nested `if` for dead letter makes the intent unclear.

### 1.2 CI Delivery Loop — No Shutdown Signal

**File:** `services/git-server/src/main.rs:996-1003`

```rust
async fn ci_delivery_loop(state: AppState) {
    loop {
        if let Err(error) = deliver_pending_ci_events(&state).await {
            tracing::warn!(error = %error, "CI outbox delivery deferred");
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}
```

The delivery loop has **no shutdown signal** — it's a fire-and-forget infinite loop. It never checks the `shutdown` flag, meaning:

- On shutdown, `http_handle.abort()` and `ssh_handle.abort()` cancel the HTTP/SSH servers, but the delivery loop and dead-letter loop keep running until their spawned tasks are implicitly cancelled when the main future completes.
- The spawned task's `JoinHandle` is never awaited or stored, so there's no way to know when it terminates.

The `main()` function awaits `timeout(Duration::MAX, shutdown_future)`, then aborts the HTTP/SSH handles and sleeps for 1 second. The delivery loop and dead-letter loop continue running beyond this — they are orphaned tasks with no shutdown coordination.

**Severity:** Medium. On shutdown, the delivery loop may continue for up to 2 seconds (plus query time) after the process appears stopped. Not a data corruption risk but means clean shutdown is incomplete.

### 1.3 Dead Letter Loop — Same Shutdown Issue

**File:** `services/git-server/src/main.rs:745-772`

```rust
async fn dead_letter_cleanup_loop(pool: Arc<gitforge_db::Pool>) {
    const PURGE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600); // 1 hour
    loop {
        tokio::time::sleep(PURGE_INTERVAL).await;
        // ... deletion query
    }
}
```

Same issue — no shutdown signal. Additionally, the loop runs every 60 minutes unconditionally, even if no dead letter entries exist. This is a minor resource concern but not a correctness issue.

**Severity:** Low.

### 1.4 CI Trigger Insert — Unbounded Background Retry

**File:** `services/git-server/src/main.rs:816-873`

When the inline retry budget (`CI_TRIGGER_SYNC_WINDOW`, default 300s) is exhausted, a background task spawns that retries with a fixed `max_backoff` (30s) for up to `GITFORGE_CI_TRIGGER_DEAD_LETTER_AFTER` (default 48 hours).

Each failed delivery spawns **another** background task. If 10 pushes fail during a database outage, 10 background tasks accumulate. Each one burns a 30s backoff sleep and an SQL insert attempt every 30 seconds. During a long outage this is manageable, but during a partial outage where some pushes succeed and some fail, the number of background tasks is unbounded.

**Severity:** Medium-High. Under sustained failure, this can accumulate unbounded background tasks. Each task holds a `Pool` clone, and the dead-letter table write on timeout adds another SQL query.

### 1.5 `unwrap_or_else` in `ci_http_client()` Swallows TLS Build Errors

**File:** `services/git-server/src/main.rs:886-891`

```rust
fn ci_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}
```

If the `Client::builder()` fails (e.g., system is missing TLS roots), it falls back to `Client::new()` with **no timeout**. The comment explains why a timeout was added, but the fallback silently removes it. Any subsequent use of this client (the CI delivery loop's HTTP requests) will have no timeout.

**Severity:** Medium. In a misconfigured system, CI triggers will never time out.

### 1.6 `deliver_pending_ci_events()` — `?` Propagates Errors, Skipping Remaining Batch

**File:** `services/git-server/src/main.rs:905-913`

```rust
let events = sqlx::query_as::<_, (String, String)>(...)
    .bind(&now_text)
    .fetch_all(pool.pool())
    .await?;
```

The `?` on `fetch_all` means if the query fails, the entire batch of up to 50 events is skipped. The events that could have been delivered are left pending until the next delivery loop iteration (up to 2 seconds later). This is acceptable for reliability (fail closed) but means a query failure temporarily stalls delivery of all pending events.

**Severity:** Low.

### 1.7 Dead Letter Query Uses Positional Bind Without Column Name

**File:** `services/git-server/src/main.rs:753`

```rust
let result = sqlx::query("DELETE FROM ci_trigger_dead_letter WHERE created_at < ?")
    .bind(&cutoff)
    .execute(pool.pool())
    .await;
```

The `DELETE` uses raw SQL with positional bind. While functionally correct, it bypasses the query builder. This is consistent with the rest of the codebase which uses raw SQL throughout.

**Severity:** None (cosmetic).

---

## 2. SSH Server (`services/git-server/src/ssh_server.rs`)

### 2.1 Channel Process — Unbounded Pending Push Buffer

**File:** `services/git-server/src/ssh_server.rs:147-151`

```rust
struct PendingPush {
    buffer: Vec<u8>,
    repo_id: gitforge_common::RepoId,
}
```

The `PendingPush` buffer accumulates incoming data until the command list is complete. There is a check at line 377 for `MAX_COMMAND_LIST_BYTES` (1 MiB), which bounds the buffer. However, if the client sends data after the command list but before `receive_push_data` drains it, that data is appended to the existing buffer. The buffer is then replayed to the git child's stdin at line 442.

The guard at line 377 is checked **before** the buffer is taken out, so once the command list is found, the buffer is not re-checked. If the client sends a very large pack (multi-hundred MB), the buffer contains the command list prefix plus potentially a large pack payload. The 1 MiB check prevents the buffer from growing unbounded **during command list accumulation**, but once the command list is found, the remaining buffer (which includes pack data that arrived before policy evaluation) is passed through.

This is not a resource leak because the policy evaluation is fast, and the data flows to the child stdin. The buffer is bounded by the client's send rate and the time between command list completion and policy evaluation (microseconds).

**Severity:** None (well bounded in practice).

### 2.2 `pump_until_exit` — CI Trigger in SSH Path Swallowed on Channel Error

**File:** `services/git-server/src/ssh_server.rs:736-741`

```rust
for update in push_event.updates {
    if !accepted_refs.contains(&update.ref_name) {
        continue;
    }
    if let Err(error) =
        crate::enqueue_ci_event(&pool, repo_id, &update, pusher_id).await
    {
        tracing::error!(?channel, ref_name = %update.ref_name, %error,
            "SSH CI trigger insert deferred to background redelivery");
    }
}
```

CI trigger insertions for SSH pushes go through the same `enqueue_ci_event` as HTTP, which means they also spawn background retry tasks. These are spawned from within the `pump_until_exit` task, which itself is spawned from `serve_git_command`. If `pump_until_exit` returns, the spawned tasks continue independently — which is the intended behavior for the dead-letter retry path.

**Severity:** Low. The spawned tasks are correct — this is by design.

### 2.3 SSH Session — `PoisonError::into_inner` on Channel Process Map

**File:** `services/git-server/src/ssh_server.rs:165`

The `processes: HashMap<russh::ChannelId, ChannelProcess>` is **not** protected by any mutex. Each SSH session (`GitSshSession`) is per-connection and `russh::Handler` methods are called on the same session instance, so there's no concurrent access to the HashMap within a single connection. However, if russh ever calls handler methods from different tasks (which it doesn't based on the single-threaded event loop), this would be a data race. This is an architectural assumption about russh's threading model, not a current issue.

**Severity:** Low.

### 2.4 `parse_receive_status_ok_refs` — No Bounds on Pkt-Line Parsing

**File:** `services/git-server/src/ssh_server.rs:811-847`

The `parse_status_packets` function walks pkt-lines recursively. If a receive-pack produces a deeply nested side-band stream, this could cause stack overflow via recursive calls at line 833:

```rust
if parse_status_packets(&payload[1..], accepted)? {
```

A real receive-pack status report is bounded (a few hundred refs), so this is not a practical concern. However, the function has no depth limit.

**Severity:** Low.

---

## 3. CI Orchestrator (`services/ci/src/main.rs`)

### 3.1 Event Consumer — Unbounded Lane Map

**File:** `services/ci/src/main.rs:2275-2276`

```rust
type EventLaneKey = (gitforge_common::RepoId, String);
type LaneMap = HashMap<EventLaneKey, tokio::sync::mpsc::UnboundedSender<EventEnvelope>>;
```

The `LaneMap` is keyed by `(repo_id, ref_name)`. Every new ref (e.g., `refs/heads/feature/...`) creates a new channel. Over time, this grows unbounded — every branch ever pushed creates a lane entry. The `or_insert_with` at line 2401 creates a new lane per (repo, ref) pair.

A long-lived process on a repository with many branches could accumulate thousands of lane entries, each holding an `UnboundedSender`. While the senders themselves are cheap (mpsc channel handles), the accumulated memory and the dispatch overhead are non-zero.

**Severity:** Medium. Over months, a busy repository with many branches accumulates unbounded lane entries.

### 3.2 Pipeline Registry — No Size Limit

**File:** `services/ci/src/main.rs:40-41`

```rust
type PipelineCache = HashMap<gitforge_common::RepoId, PipelineDefinition>;
type PipelineRegistry = HashMap<gitforge_common::PipelineRunId, Arc<CiEngine>>;
```

Both maps are unbounded. The `PipelineRegistry` lives for the process lifetime and holds one `CiEngine` per run. As runs complete and engines are finalized (line 3183: `registry.write().await.remove(&state.run_id)`), entries are removed. However, if `finalize_run_if_terminal` fails to remove an engine from the registry (e.g., the `update_status` write fails at line 3152 and returns early), that engine entry leaks until process restart.

**Severity:** Medium. Engine leak on finalization failure.

### 3.3 Workspace Paths — PoisonError on Mutex

**File:** `services/ci/src/main.rs:2667-2668, 2670-2673, 2827-2829`

Throughout `handle_push_event`, mutex accesses use:
```rust
workspace_paths
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner)
```

This discards the panic indicator and proceeds. If a worker thread panics while holding the mutex, the next access silently proceeds with the poisoned lock. This is a standard pattern in Rust but means a single panicked worker can cause subsequent workers to read stale workspace path data without notification.

**Severity:** Low.

### 3.4 Run Waiters — Potential Leak on Bus Publish Failure

**File:** `services/ci/src/main.rs:1057-1062`

```rust
let (run_tx, run_rx) = tokio::sync::oneshot::channel();
trigger_state
    .run_waiters
    .lock()
    .unwrap_or_else(PoisonError::into_inner)
    .insert(event.event_id, run_tx);
```

The waiter is inserted into the map, and removed in the completion path (`run_waiters.remove(&event.event_id)` at lines 2485, 2497, 2504). However, if the event consumer exits (bus stream closes permanently, or the consumer task panics), the waiters are never cleaned up. On restart, a new process starts with a clean `run_waiters` map, so this is bounded per-process.

**Severity:** Low.

### 3.5 Trigger Sweep — Stale Threshold May Double-Run Commits

**File:** `services/ci/src/main.rs:71-72`

```rust
const TRIGGER_PROCESSING_STALE_AFTER_SECS: i64 = 30 * 60;
```

The 30-minute threshold for demoting a `processing` trigger to `pending` is generous (the comment notes workspace clones can take ~10 minutes). However, if a trigger is genuinely processing (e.g., a very large repository clone is in progress), demoting it creates a duplicate trigger that races the original. The dedup key in `create_or_existing` prevents duplicate runs, but it means extra work for the second trigger.

**Severity:** Low.

### 3.6 Object Cache — No Cleanup of Orphaned Cache Dirs

**File:** `services/ci/src/main.rs:2117-2119`

```rust
fn object_cache_path(repo_id: gitforge_common::RepoId) -> std::path::PathBuf {
    workspace_root().join("_objects").join(repo_id.to_string())
}
```

Object caches are created per-repository but never cleaned up. When a repository is deleted, its object cache directory remains on disk. Over time, this accumulates. The `_objects` directory under `workspace_root()` has no retention policy.

**Severity:** Medium. Unbounded disk accumulation for deleted repositories.

### 3.7 `finalize_run_if_terminal` — Workspace Deletion Spawned Without Timeout

**File:** `services/ci/src/main.rs:3180-3181`

```rust
tokio::spawn(async move {
    remove_run_workspace_dir(&root, run_id, workspace_path.as_deref()).await;
});
```

The workspace cleanup is spawned without a timeout. While `remove_run_workspace_dir` has internal timeouts for individual operations (Docker cleanup, Podman cleanup), a very large workspace with many files could take an unbounded amount of time to delete, especially under I/O pressure. The spawned task holds a `JoinHandle` that is never awaited, making it fire-and-forget.

**Severity:** Medium. A large workspace delete could block indefinitely (though individual operations have timeouts, the overall duration is unbounded).

### 3.8 Reconcile Loop — `Delay` Behavior for Missed Ticks

**File:** `services/ci/src/main.rs:1847`

```rust
interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
```

The `run_reconciliation_loop` uses `Delay` for missed ticks, while the trigger sweep (line 479) and other periodic tasks use `Skip`. This means if the reconciliation pass takes longer than 60 seconds, subsequent passes are delayed, potentially accumulating backlog. Under heavy load, reconciliation could drift significantly.

**Severity:** Low.

### 3.9 `persist_planned_jobs` — Spawned Workspace Cleanup Without Await

**File:** `services/ci/src/main.rs:2858-2860`

```rust
if let Some(path) = run_workspace_paths
    .lock()
    .unwrap_or_else(PoisonError::into_inner)
    .remove(&state.run_id)
    .flatten()
{
    tokio::spawn(async move {
        let _ = tokio::fs::remove_dir_all(path).await;
    });
}
```

On failed planning, the workspace directory is cleaned up in a spawned task. The `remove_dir_all` can be very slow for large directories and has no timeout.

**Severity:** Medium.

### 3.10 CI Library Code — No TODOs or Unimplemented Items Found

Scanned the entire CI codebase. No `TODO`, `FIXME`, or `unimplemented!` markers were found.

### 3.11 CI Test Coverage Gaps

The CI test suite (`ci_trigger_flow.rs`, `ci_trigger_settlement.rs`) tests:
- End-to-end trigger flow with committed pipeline
- Settlement of unparseable configs (terminal vs retryable)
- HTTP contract for malformed requests
- Trigger token fail-closed behavior

**Not tested:**
- Engine redrive reconciliation (lines 3398-3704 of main.rs — ~300 lines of complex logic with zero unit tests)
- Fenced engine reconciliation (~80 lines — no unit tests)
- `cancel_doomed_rows` cascade logic (~60 lines — no unit tests)
- `reconcile_orphaned_runs_filtered` (~130 lines — no unit tests)
- `merge_completed_job_env` (~25 lines — no unit tests)
- `persist_skipped_job_rows` (~20 lines — no unit tests)
- `finalize_run_if_terminal` (~55 lines — no unit tests)

The test coverage for the CI orchestrator's business logic is concentrated on the trigger HTTP layer and settlement paths, while the core reconciliation and engine state management logic has no direct tests.

**Severity:** High. The most complex and failure-prone code paths have no test coverage.

---

## 4. Ref Update Policy (`services/git-server/src/ref_policy.rs`)

### 4.1 Cache Miss — Unbounded Cache Growth

**File:** `services/git-server/src/ref_policy.rs:192`

```rust
let mut cache: Vec<(String, Vec<CommitRunStatus>)> = Vec::new();
```

The commit status cache is a `Vec` appended per-push evaluation. It lives for the duration of `evaluate_ref_policy` (a single request), so it's not a memory leak. However, the cache is **per-request** — it doesn't persist across pushes, so the same commit is looked up repeatedly from the database for every push that includes it.

**Severity:** None (cache is per-request, not per-process).

### 4.2 `evaluate_required_checks` — No Unit Tests for Check Matching

**File:** `services/git-server/src/ref_policy.rs:193-243`

The loop that evaluates required checks per-commit has unit tests in `ref_policy.rs` (lines 323-585), covering the decision logic. The integration with `PipelineRunQueries` is tested in `git_ref_policy_http.rs` but the check matching logic itself has no explicit edge-case tests for partial matches, status aliases, or concurrent checks.

**Severity:** Low.

---

## 5. Architecture and Cross-Cutting Concerns

### 5.1 InMemoryEventBus — Volatile State

**File:** `crates/gitforge-events/src/bus.rs`

Both `git-server` and `ci` use `InMemoryEventBus`. Events published to this bus are lost on restart. The CI orchestrator mitigates this with a durable trigger-request table and sweep, but the git-server's `InMemoryEventBus` (line 120) has no durable backup — if an SSH push's CI trigger is published to the bus and the CI consumer is down, the event is lost. The git-server's outbox retry mitigates this by persisting to the `events` table directly.

**Severity:** Low. The git-server's outbox pattern provides durability; the bus is just an optimization path.

### 5.2 No Rate Limiting on CI Delivery Loop

**File:** `services/git-server/src/main.rs:996-1003`

The CI delivery loop fires every 2 seconds, sending up to 50 pending events per batch. There is no rate limiting on the total number of HTTP requests to the CI trigger endpoint. Under heavy load (e.g., a monorepo with hundreds of pushes), this could overwhelm the CI orchestrator.

**Severity:** Low-Medium.

### 5.3 Workspace Root Default — Development Path in Production

**File:** `services/ci/src/main.rs:1213-1218`

```rust
fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var("GITFORGE_WORKSPACE_ROOT")
            .unwrap_or_else(|_| "/var/lib/gitforge/workspaces".to_string()),
    )
}
```

The CI workspace root defaults to `/var/lib/gitforge/workspaces` when unset. The git-server's `get_git_root()` defaults to `target/gitforge-repos` (a development path). If CI and git-server run on the same host without explicit configuration, they may not share the expected workspace layout.

**Severity:** Low.

### 5.4 Dead Letter Table Migration

**File:** `migrations/002_dead_letter.sql`

The dead letter table exists in migrations but there is no explicit migration check in the git-server's startup code. The `dead_letter_cleanup_loop` uses `sqlx::query` directly, which will fail silently if the table doesn't exist.

**Severity:** Low.

---

## 6. Summary of Findings by Severity

### High
| # | Issue | File | Line |
|---|-------|------|------|
| H1 | CI orchestrator's core reconciliation logic (~450 lines) has zero unit tests | `services/ci/src/main.rs` | 3398-3704, 2884-2934, 1616-1744, 3124-3184 |

### Medium-High
| # | Issue | File | Line |
|---|-------|------|------|
| M1 | Unbounded background retry tasks accumulate during sustained DB failures | `services/git-server/src/main.rs` | 816-873 |
| M2 | Object cache dirs for deleted repositories are never cleaned up | `services/ci/src/main.rs` | 2117-2119 |

### Medium
| # | Issue | File | Line |
|---|-------|------|------|
| M3 | CI delivery loop has no shutdown signal | `services/git-server/src/main.rs` | 996-1003 |
| M4 | Unbounded lane map grows with every new repo/ref combination | `services/ci/src/main.rs` | 2275-2276 |
| M5 | PipelineRegistry can leak entries on finalization failure | `services/ci/src/main.rs` | 3183 |
| M6 | `ci_http_client()` fallback removes timeout entirely | `services/git-server/src/main.rs` | 886-891 |
| M7 | Workspace deletion in `finalize_run_if_terminal` has no overall timeout | `services/ci/src/main.rs` | 3180-3181 |
| M8 | Spawned workspace cleanup in planning failure path has no timeout | `services/ci/src/main.rs` | 2858-2860 |

### Low
| # | Issue | File | Line |
|---|-------|------|------|
| L1 | Dead letter cleanup loop has no shutdown signal | `services/git-server/src/main.rs` | 745-772 |
| L2 | Delivery loop `?` skips remaining batch on query failure | `services/git-server/src/main.rs` | 905-913 |
| L3 | Reconcile loop `Delay` behavior under load | `services/ci/src/main.rs` | 1847 |
| L4 | No rate limiting on CI delivery loop | `services/git-server/src/main.rs` | 996-1003 |
| L5 | Stale trigger threshold may cause duplicate triggers | `services/ci/src/main.rs` | 71-72 |

---

## 7. Recommendations

1. **Add shutdown coordination to background loops.** Both `ci_delivery_loop` and `dead_letter_cleanup_loop` should accept a `shutdown: Arc<AtomicBool>` and check it in their loop bodies.

2. **Cap the background retry task count.** Implement a simple LRU or FIFO cap (e.g., max 100 background retry tasks per process) to prevent unbounded accumulation during sustained outages.

3. **Add unit tests for CI reconciliation logic.** The ~450 lines of reconciliation, engine rebuild, and orphan run finalization logic are the most complex code paths and have no tests. Prioritize `reconcile_live_engines`, `cancel_doomed_rows`, and `reconcile_orphaned_runs_filtered`.

4. **Add cleanup for orphaned object caches.** A periodic sweep that removes object caches whose corresponding repository rows have been deleted would prevent unbounded disk growth.

5. **Fix `ci_http_client()` fallback.** Instead of `unwrap_or_else`, use `expect` with a clear error, or log a warning and create a client without timeout while clearly documenting the degraded state. Alternatively, use a builder that always succeeds with a default timeout.

6. **Cap the lane map.** Implement a TTL-based eviction or an LRU cap (e.g., 1000 lanes) for the event lane map. Stale lanes for inactive refs accumulate indefinitely.

7. **Add timeout to workspace cleanup spawns.** Wrap `remove_run_workspace_dir` in a `timeout()` call (e.g., 5 minutes) to prevent unbounded cleanup durations.

8. **Add rate limiting to CI delivery loop.** Consider a token bucket or semaphore to limit the rate of HTTP requests sent to the CI orchestrator.
