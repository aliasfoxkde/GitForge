# Quality Baseline — Unwrap Inventory (2026-10-10)

Two counts, two purposes:

- **Raw** (`scripts/unwrap-count.sh`): line-based grep over each crate's
  `src/`, including `#[cfg(test)]` modules and everything else. Trend
  metric only — test code legitimately unwraps.
- **Production** (cfg-test-aware categorization, via
  `scripts/unwrap-production-count.py`): sites outside `#[cfg(test)]`
  items and outside `tests/` directories, counted only inside code
  regions (string-literal, char-literal, and comment contents are
  blanked before matching, so pattern text inside `format!` strings or
  doc comments never counts as a site).

## Campaign status: production surface drained (2026-10-10)

The workspace production count went **112 → 5** in four batches
(3744f00c, 91f414c6, and the batch-3/enforcement commit), validated
through the GitForge pipeline on the fedora remote. The five
residuals are deliberate or documented:

| sites | location | status |
|---:|---|---|
| 2 | `services/api/src/main.rs` (JWT_SECRET_FILE aborts) | deliberate fail-fast startup aborts in a binary main |
| 1 | `crates/gitforge-api/src/metrics.rs` (`new()`) | documented invariant: static metric names, fresh registry |
| 1 | `crates/gitforge-review/src/lib.rs` (`static_regex`) | documented invariant: compile-time pattern table |
| 1 | `crates/gitforge-process/src/pool.rs` (`acquire`) | documented invariant: semaphore is never closed |

## Enforcement (active)

`[workspace.lints.clippy]` now carries `unwrap_used = "deny"` and
`expect_used = "deny"`, inherited by every crate via
`[lints] workspace = true`. `clippy.toml` sets
`allow-unwrap-in-tests` / `allow-expect-in-tests` so the ~3200
test-context sites stay legal — assertions are the one place a panic
is the desired failure mode. The three documented production expects
survive the deny via per-site `#[allow]` with their invariants written
next to them; any NEW undocumented site fails CI.

## Measurement history

- **First cut (superseded):** 191 "production" sites. The categorizer's
  lexer mishandled raw string literals (`r#"…"#`) and mis-gated char
  literals against lifetimes, truncating `#[cfg(test)]` brace spans
  early — e.g. it reported 71 "production" sites in
  `gitforge-runner/agent.rs` that are all inside test modules.
- **Lexer fix:** 112 production / 3193+ test, top-15 table summing
  exactly; cross-checked per file with line-bound greps against each
  file's `#[cfg(test)]` marker.
- **String/comment-aware counting (final):** two more false positives
  removed — a `.expect(` inside a `format!` literal (`fix.rs`) and one
  in a doc comment (`subreaper.rs`). Final production count 5.
- Raw per-crate grep totals (2026-10-10) for the trend ratchet:
  2135 sites (1933 unwrap / 185 expect / 17 panic), 93–95%
  test-context.

## Conversion idioms (for future code)

1. **Fallible parsing/registration** → propagate `?` (`Metrics::build`,
   `MockHttpClient` round-trips).
2. **Static pattern tables** (regex, metrics) → single helper with one
   documented expect (`static_regex`, `Metrics::new`); failure names
   the offending literal and is a table defect, not a runtime
   condition.
3. **Mutex poisoning** on coordination locks guarding rebuildable
   in-memory state → `unwrap_or_else(PoisonError::into_inner)`
   (availability over escalation; the durable store is the source of
   truth).
4. **Owned invariants across `Option` fields** → checked preconditions
   (`GitRpcChild::owned_child` → `Result`).
5. **Detached serve tasks** → `unwrap_or_else` + `tracing::error!`
   (a panic in a spawned task dies silently in the JoinHandle).
6. **True fail-fast startup aborts in binaries** (unreadable required
   secret) → keep the `panic!` with a contextual message.

## Coverage ratchet (separate ledger item)

Requires an in-sandbox measurement before raising thresholds; not
calibrated this cycle.
