# M7 — Toolchain & Test-Runner Benchmark Plan (issue #341)

Goal: measure where CI wall-time actually goes in the toolchain stages
(fmt, clippy, build, test) and quantify the levers we control: target-dir
placement (NVMe vs NAS), warm vs cold dependency state, and shared cache
mounts (sccache-style).

## Experiment matrix

| Cell id | Command | Target dir | State | Hypothesis |
|---------|---------|-----------|-------|------------|
| `fmt-clippy-cold` | `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings` | shared NVMe | cold (fresh crate artifacts) | clippy cold dominates; ~10-20 min on 16 cores |
| `fmt-clippy-warm` | same | shared NVMe | warm (artifacts present) | clippy warm < 3 min; incremental check is cheap |
| `build-common-cold` | `cargo build -p gitforge-common` | shared NVMe | cold (crate artifacts cleaned) | leaf crate build is dep-dominated, not crate-dominated |
| `build-common-warm` | same | shared NVMe | warm | seconds; confirms incremental value |
| `build-nas-target` | `cargo build -p gitforge-common` | NAS scratch target | warm-ish | NAS target dir is 5-50x slower per metadata op; btrfs open stalls |
| `test-single-warm` | `cargo test -p gitforge-common` | shared NVMe | warm | single-crate test is minutes, not the workspace's tens of minutes |

Deferred cells (need the .202 sandbox or a pipeline change to be
meaningful):

- full workspace `cargo test` — not run on the NAS host under load
  (agent brief rule); to be measured on .202 against the in-sandbox
  coverage baseline (docs/audits/COVERAGE_BASELINE_2026-10-10.md).
- sccache-style cache mounts — dsc-ci-rust has no sccache today; the
  experiment requires a pipeline/image change (see Recommendation).
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` timing,
  since the CI clippy job chains rustdoc after it.

## Measurement procedure

1. Driver: `scripts/bench-toolchain.sh <cell-id> -- <command...>`.
   It captures env at start into variables (TARGET_DIR, TMPDIR, HOST,
   GIT_SHA) and never re-reads the environment inside the timed
   section — measurement only, no decision logic.
2. Timing: wall clock around the whole command via `date +%s.%N`; the
   UTC timestamp is captured once at script start and reused for all
   output fields (edge-captured, passed through, not re-read).
3. Output: one JSON object per cell on stdout and to
   `docs/planning/m7-raw/<cell-id>.json`: cell id, command, wall time
   seconds, exit code, host, timestamp, git sha, load average.
4. Cold/warm discipline: "cold" = first run after cleaning the subject
   crate's artifacts (deps stay; a full dependency-cold build of 15
   crates is one measurement class, not five repeats). "Warm" = second
   consecutive run of the identical command.
5. Every run uses `timeout 900` (15 min cap) and
   `CARGO_TARGET_DIR=/var/tmp/gitforge-target TMPDIR=/tmp` unless the
   cell explicitly varies the target dir.
6. Record co-tenant load (16-core host shared with other agents): grab
   `uptime` load average before each cell into the JSON as a confound
   marker.

## Acceptance criteria

- Each cell has >=1 cold and >=1 warm measurement with real numbers in
  the results doc; no projected/estimated values.
- JSON artifacts stored under `docs/planning/m7-raw/` and referenced
  from `M7_BENCHMARK_RESULTS.md`.
- Confounds (load average, co-tenant builds, NAS contention) are stated
  per cell, not hand-waved globally.
- The results doc ends with concrete, actionable recommendations for
  the CI image / pipeline, each tied to a measured delta, not intuition.

## Known confounds

- This host is shared; other agents run cargo builds against the same
  `/var/tmp/gitforge-target` concurrently. Load average is captured per
  cell; single-crate cells are small enough that contention shows as
  variance, and we note rather than average it away.
- NAS reads spin up the JBOD; first-touch NAS cells include spin-up.
- The shared target dir means "cold" here is crate-cold, not
  dependency-cold, unless noted.
