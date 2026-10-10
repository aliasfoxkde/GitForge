# Quality Baseline — Unwrap Inventory (2026-10-10)

Two counts, two purposes:

- **Raw** (`scripts/unwrap-count.sh`): line-based grep over each crate's
  `src/`, including `#[cfg(test)]` modules and everything else. Trend
  metric only — test code legitimately unwraps.
- **Production** (cfg-test-aware categorization, 2026-10-10, via
  `scripts/unwrap-production-count.py`): sites outside `#[cfg(test)]`
  items and outside `tests/` directories. This is the elimination
  scope: **112 sites workspace-wide** (the raw 2135 is 95%
  test-context).

  Measurement caveat (fixed same day): the first cut of the
  categorizer's lexer mishandled raw string literals (`r#"…"#`) and
  mis-gated char literals against lifetimes, which truncated
  `#[cfg(test)]` brace spans early — e.g. it reported 71 "production"
  sites in `gitforge-runner/agent.rs` that are all inside its test
  modules. After the lexer fix the workspace count dropped 191 → 112,
  and the top-15 table below sums to exactly 112 (no hidden tail).
  Cross-checked per file with a line-bound grep against each file's
  `#[cfg(test)]` marker (metrics.rs 32, security.rs 20, ci/main.rs 14,
  receipt_store.rs 10 — all match exactly).

This file is the ratchet: either count may go down or hold, never up.
When a crate's *production* count reaches zero, its `Cargo.toml` gains
the enforcement lints (`clippy::unwrap_used`, `clippy::expect_used` =
"deny" for library crates; "warn" where a binary's main legitimately
expects) under the workspace `[lints]` inheritance.

Per the 2026-10-10 operating directive, CI runs on the GitForge instance
on the fedora remote; the elimination batches are validated there, one
crate-batch per pipeline run.

## Production sites (the campaign scope), by file

| site count | file |
|---:|---|
| 32 | crates/gitforge-api/src/metrics.rs |
| 20 | crates/gitforge-review/src/security.rs |
| 14 | services/ci/src/main.rs |
| 10 | crates/gitforge-storage/src/receipt_store.rs |
| 6  | crates/gitforge-review/src/lib.rs |
| 6  | crates/gitforge-review/src/fix.rs |
| 5  | crates/gitforge-process/src/pool.rs |
| 4  | crates/gitforge-storage/src/job_logs.rs |
| 4  | crates/gitforge-core/src/git_protocol/http.rs |
| 4  | crates/gitforge-cli/src/sync.rs |
| 2  | services/api/src/main.rs |
| 2  | crates/gitforge-core/src/hooks.rs |
| 1  | services/git-server/src/main.rs |
| 1  | crates/gitforge-process/src/subreaper.rs |
| 1  | crates/gitforge-ci/src/state.rs |
| **112** | **workspace total** (the table is exhaustive) |

Raw per-crate counts (grep, includes test context — superseded as the
campaign scope by the table above, kept for the trend ratchet):

| crate | unwrap | expect | panic | total |
|---|---:|---:|---:|---:|
| gitforge-ai | 6 | 0 | 0 | 6 |
| gitforge-api | 95 | 33 | 1 | 129 |
| gitforge-build | 29 | 17 | 8 | 54 |
| gitforge-ci | 143 | 2 | 0 | 145 |
| gitforge-cli | 85 | 1 | 0 | 86 |
| gitforge-common | 14 | 0 | 0 | 14 |
| gitforge-core | 78 | 20 | 0 | 98 |
| gitforge-db | 470 | 2 | 0 | 472 |
| gitforge-events | 36 | 1 | 0 | 37 |
| gitforge-process | 7 | 8 | 1 | 16 |
| gitforge-review | 44 | 1 | 0 | 45 |
| gitforge-runner | 64 | 32 | 2 | 98 |
| gitforge-sandbox | 87 | 5 | 0 | 92 |
| gitforge-scheduler | 238 | 7 | 0 | 245 |
| gitforge-storage | 260 | 4 | 0 | 264 |
| api (service) | 15 | 0 | 2 | 17 |
| ci (service) | 222 | 33 | 0 | 255 |
| git-server (service) | 36 | 18 | 3 | 57 |
| runner (service) | 4 | 1 | 0 | 5 |
| **TOTAL** | **1933** | **185** | **17** | **2135** |

## Elimination order (production sites only)

1. `gitforge-api/metrics.rs` (32), `gitforge-review/security.rs` (20) —
   parsing and analysis paths; convert to `Result` propagation. First
   batch (the original first batch, `gitforge-runner/agent.rs`, was a
   miscount — all 99 of its sites are test-context).
2. `services/ci/main.rs` (14), `storage/receipt_store.rs` (10) —
   startup and dispatch paths; `main()`-adjacent unwraps may convert
   to documented `expect` with context, not silent `unwrap`.
3. `review/lib.rs` + `review/fix.rs` (12) and `process/pool.rs` (5) —
   review analysis and child-process plumbing.
4. The long tail (≤4 each, 28 sites) — one closing batch.
5. `gitforge-db`, `gitforge-storage`, `gitforge-scheduler`, `gitforge-ci`
   raw counts are 93-97% test context — their production sites are in
   the files above; no separate crate campaigns needed.

## Related foundation

- Root `[workspace.lints]` (this cycle): `rust_2018_idioms`,
  `unsafe_op_in_unsafe_fn`, `clippy::dbg_macro`,
  `clippy::unimplemented` at warn — safe under the CI `-D warnings`
  gate. Enforcement-grade restriction lints attach per crate with the
  campaign, never workspace-wide ahead of it.
- Coverage ratchet (separate ledger item) requires an in-sandbox
  measurement before raising thresholds; not calibrated this cycle.
