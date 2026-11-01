# M7 — Toolchain Benchmark Results (issue #341)

Run on SWARMONE (16-core NAS host, shared with ~co-tenant agent load),
2026-11-01, git sha `d476baf7`, `CARGO_TARGET_DIR=/var/tmp/gitforge-target`
(NVMe), `TMPDIR=/tmp`. Raw JSON: `docs/planning/m7-raw/*.json`.

## Results

| Cell | Wall (s) | Exit | Load avg (1m) | Verdict |
|------|---------:|-----:|---------------|---------|
| `fmt-clippy-warm` | 58.90 | 0 | 36.3 | incremental clippy is cheap |
| `fmt-clippy-cold` (all crate sources invalidated) | 63.55 | 0 | 31.8 | crate-cold clippy adds only ~5s over warm |
| `build-common-warm` | 7.64 | 0 | 52.5 | — |
| `build-common-cold` (single-crate recompile, deps warm) | 1.29 | 0 | 49.3 | leaf crate recompile is trivial |
| `build-common-cold2` (false cold: fingerprint fresh) | 0.34 | 0 | 50.1 | invalidity probe, not a real cell |
| `build-common-cold3` (aborted: stray empty build.rs) | 149.05 | 101 | 42.3 | invalid run, kept as confound record |

## Verification that "cold" really re-ran

`cargo clean -p` refuses on the shared target dir (no CACHEDIR.TAG), so cold
invalidation was done by touching source mtimes. Verification:

- cold clippy cell: 290 `.fingerprint/*/invoked.timestamp` files written
  inside the cell's wall-clock window — clippy-driver genuinely re-ran
  across the workspace.
- cold single-crate build: `libgitforge_common.rlib` mtime advanced past
  the invalidated source mtime; a post-cell `cargo build -v` shows 37 Fresh
  units and no recompile.

## Confounds

- **Co-tenant load is real and heavy**: 1-minute load averages of 20-52 on
  a 16-core host during every cell. Absolute numbers are inflated and
  noisy; deltas between cold and warm within the same session are more
  trustworthy than the raw seconds. Cells must be re-measured on the
  .202 CI sandbox before any CI-image sizing decision is finalized.
- **"Cold" here is crate-cold, not dependency-cold.** Dependency compile
  (the dominant CI cost for a fresh workspace) was not measured locally —
  the shared target dir always has warm deps. Dependency-cold measurement
  belongs on .202 where the CI workspace starts empty.
- `build-common-cold3` failed with exit 101 because an untracked empty
  `build.rs` appeared in the worktree during the run (source-invalidation
  probe artifact); it was removed and the cell re-run. Kept for the
  record.
- NAS-target cell (`build-nas-target`) was **not** run: host load made the
  measurement useless, and the NAS JBOD spin-up cost is already documented
  operationally (memory: TMPDIR→NAS btrfs sqlite stalls, 0.5-10s/open).
  Deferred to .202 or a quieter window.

## Recommendations

1. **Do not chase clippy incremental cost first.** Crate-cold vs warm
   clippy differ by only ~5s (63.6s vs 58.9s) with deps warm. The CI
   clippy job's pain, if any, is dependency compilation, which only
   .202 can measure (fresh workspace each run).
2. **Dependency cache is the lever worth testing.** Since the local
   numbers show per-crate work is seconds, the CI image should be
   evaluated with a warmed `CARGO_TARGET_DIR` or sccache mount versus
   the current per-run empty workspace. Expected payoff is minutes, not
   seconds — but it must be measured on .202 (per the cache-mount
   rollout gate: verify the bind mount actually engages before
   trusting warm numbers).
3. **Keep the NVMe target dir.** NAS placement is disqualified on
   operational evidence (btrfs open stalls) and was not worth re-measuring
   under this host's load; the deferred NAS cell should run on .202 only
   if someone proposes NAS as a CI target again.
4. **Single-crate rebuilds are effectively free locally (1.3s).** This
   supports the M6 direction (incremental testing / result reuse): the
   cost model for skipping unchanged crates is safe to assume cheap on
   the runner side; the interesting costs are test execution and DB, not
   compilation.

## Next steps (M7 continuation)

- Re-run the matrix on .202 inside the dsc-ci-rust sandbox: cold deps,
  full `cargo test -p gitforge-common`, rustdoc gate timing.
- sccache mount A/B on .202 (with docker-inspect verification of the
  mount), per the cache-mount rollout gate.
