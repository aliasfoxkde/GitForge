# PR #294 follow-up receipt — typed retry classification of the git-path preflight (2026-10-11)

- **Branch**: `codex/gitforge-preflight-db-classification-20261011`, stacked on
  `codex/gitforge-pr293-trigger-contract-20261011` @ `6228f868` (PR #294 head,
  verified identical to origin before work started). Draft PR, left unmerged.
- **Scope of fix**: `services/ci/src/main.rs` only — the git-path preflight
  gate in `handle_push_event`, the shared resolver
  `resolve_repository_git_path`, one new marker type + classifier, and unit
  tests. Error strings unchanged. This receipt is the only docs change.
- **Trigger**: independent review of PR #294. The ghost-run gate mapped
  *every* `resolve_repository_git_path` error to
  `TriggerFailure::terminal`, but the resolver also propagates transient
  `RepoQueries` DB errors via `?`. A temporary DB busy / pool error at the
  gate (which reads *after* the committed-config loader's own DB read, so
  pool exhaustion between the two reads is real contention) would settle the
  trigger request terminally and burn its retry budget on a failure a retry
  can plausibly clear.

## Exact classification rules

`resolve_repository_git_path` failures, and only these, flow through
`classify_repository_path_failure`:

| Failure | Typed as | `TriggerFailure` | Durable rows at failure |
|---|---|---|---|
| `RepoQueries::get` returns `Err` (DB busy, pool exhausted/closed, connection lost) | `RepositoryLookupUnavailable(gitforge_common::Error)` wrapped in `anyhow::Error` | **retryable**, `run_id: None` | none |
| Repo row absent ("repository … is not registered") | untyped `anyhow` | terminal, `run_id: None` | none |
| `canonicalize` fails ("git path is unavailable …") | untyped `anyhow` | terminal, `run_id: None` | none |
| Resolved path is not a directory | untyped `anyhow` | terminal, `run_id: None` | none |

- **Mechanism**: `anyhow::Error::is::<RepositoryLookupUnavailable>()`
  downcast — type-based, no error-string matching (same idiom as the
  existing `InvalidPipelineConfig` classification at the config loader).
- **Untouched by design**: `prepare_run_workspace`, the resolver's other
  caller, keeps `?` — its failures occur after the run row exists and stay
  `terminal_after_run` (re-running would duplicate a graded run). The marker
  rides inside `anyhow` harmlessly there. `load_pipeline_from_commit`
  keeps its existing classifications (unregistered repo → retryable via the
  loader, invalid committed config → terminal).
- Successful resolution path unchanged: same `Ok(canonicalized path)`.

## Regression tests (red/green)

All in `mod tests` of `services/ci/src/main.rs`:

1. `test_resolve_repository_git_path_types_db_lookup_error_as_transient` —
   a closed pool (deterministic `ErrorKind::Database` "pool closed" failure,
   no timing dependence) must type the resolver error as
   `RepositoryLookupUnavailable`; the same pool's registered-but-missing git
   path (control) must **not** carry the marker.
2. `test_classify_repository_path_failure_splits_transient_lookup_from_definitive`
   — the classifier returns retryable + `run_id: None` for a typed DB
   lookup failure (cause stays diagnosable on the failure) and terminal +
   `run_id: None` for a definitive unavailable-path error.
3. `test_handle_push_event_retries_db_failure_before_run_creation` —
   end-to-end seam contract: with a dead pool, `handle_push_event` returns
   a **retryable** failure with no run id, an empty pipeline cache, and no
   prepared workspace.

- **RED** (before the fix): the typed boundary did not exist —
  `cargo test -p ci --bin ci <new tests>` failed to compile with 5 ×
  E0425 (`cannot find function classify_repository_path_failure`,
  `cannot find type RepositoryLookupUnavailable`). A compiling RED is not
  available for the transient case: see the documented limit below.
- **GREEN** (after the fix): all three new tests pass, and the preserved
  suites pass unchanged —
  `test_handle_push_event_fails_terminal_without_run_for_missing_git_path`
  (terminal, zero run rows, no workspace), `…_ignores_ref_deletion`
  (zero-run), all 6 `prepare_run_workspace_*` (resolver callers), all 6
  `test_load_pipeline_from_commit_*`.

### Documented limit

A transient DB failure *isolated to the gate's own lookup* cannot be
deterministically injected at the `handle_push_event` seam: both DB reads
(the committed-config loader's and the gate's) share one pool, so any
pool-level failure surfaces at the loader first — which already classified
retryable before this fix. Test 3 is therefore a **reorder guard**: it pins
the observable contract "a DB-layer failure before run creation keeps the
retry budget and mints no durable trace" so that if the gate is ever moved
ahead of the loader, its classification path still answers retryable there.
The transient-vs-definitive split itself is pinned directly at the typed
boundary (tests 1–2). A live SQLITE_BUSY is not producible on demand
without stalling the test for the pool's 30 s busy timeout (WAL readers
otherwise don't block), and no sleeps, retries, or simulated errors were
added. The zero-run-row assertion is out of reach with a closed pool (the
in-memory database dies with it); test 3 asserts `run_id: None` plus empty
cache/workspace, and the missing-path test pins the zero-rows shape against
a live pool.

## Bounded verification (this NVMe worktree, `TMPDIR=/tmp`,
`CARGO_TARGET_DIR=/var/tmp/cargo-target-preflight`)

- `cargo fmt --all -- --check` — clean (after formatting the new tests).
- `cargo clippy -p ci --all-targets -- -D warnings` — clean.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p ci --no-deps` — clean.
- `cargo test -p ci --bin ci -- <focused filters>` — **17/17 passed**
  (1.05 s).
- `cargo test -p ci --test ci_trigger_flow` — **3/3 passed** (1.59 s),
  including both PR #294 contract tests.

Not run locally per tasking constraint (focused tests only): the full
workspace test/clippy/coverage sweep — delegated to the checks that run on
the draft PR.

## Remaining risk

- The retryable path through the gate itself is verified at the typed
  boundary and via the reorder guard, not by an isolated end-to-end
  transient injection (impossible at this seam without fault-injection
  machinery; see limit above).
- In the race the fix targets (loader read succeeds, gate read fails), the
  request now retains its retry budget; a retry that lands after the repo's
  git path is genuinely retired will then settle terminal on the definitive
  branch — same final state as before, one extra attempt spent.
- Hosted checks (GitHub Actions + self-hosted GitForge pipeline) have not
  been observed green at this head yet; the PR is a draft until they are.
