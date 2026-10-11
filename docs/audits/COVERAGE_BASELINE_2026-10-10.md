# Workspace Coverage Baseline — 2026-10-10

Measured at `c4e3816b` (post-v0.6.16 main) inside the CI sandbox the gate
runs in: fedora (192.168.1.202), `dsc-ci-rust:10`, the exact gate command
(`cargo llvm-cov --workspace --all-targets --summary-only --fail-under-lines
82 -- --test-threads=2`). Gate verdict at this commit: **GATE-RC=0**.

## Headline numbers

| Metric | Value |
|---|---|
| Line coverage (the gate's `$10` column) | **83.04%** (44910 lines, 7616 missed) |
| Region coverage | 83.71% (66929 regions, 10902 missed) |
| Functions executed | 80.49% of 4982 |
| Gate | fail < 82%, warn < 84% (`.gitforge.yml` coverage job) |

The gate's awk reads `$10`, which the llvm-cov text header confirms is the
**Lines Cover** column (columns are Filename, Regions×3, Functions×3,
Lines×3, Branches×3) — the gate measures what its name claims.

## Per-crate lines coverage (worst first)

| Crate | Lines | Missed | Cover |
|---|---:|---:|---:|
| services/git-server | 1775 | 983 | 44.62% |
| services/runner | 207 | 82 | 60.39% |
| crates/gitforge-runner | 3758 | 1344 | 64.24% |
| services/ci | 3901 | 1198 | 69.29% |
| crates/gitforge-cli | 3058 | 716 | 76.59% |
| crates/gitforge-sandbox | 1815 | 347 | 80.88% |
| crates/gitforge-build | 2011 | 382 | 81.00% |
| services/api | 314 | 59 | 81.21% |
| crates/gitforge-api | 6351 | 1005 | 84.18% |
| crates/gitforge-scheduler | 5070 | 489 | 90.36% |
| crates/gitforge-db | 6570 | 611 | 90.70% |
| crates/gitforge-ai | 905 | 74 | 91.82% |
| crates/gitforge-storage | 2125 | 157 | 92.61% |
| crates/gitforge-process | 650 | 40 | 93.85% |
| crates/gitforge-core | 1485 | 69 | 95.35% |
| crates/gitforge-events | 1314 | 27 | 97.95% |
| crates/gitforge-common | 691 | 9 | 98.70% |
| crates/gitforge-ci | 1904 | 19 | 99.00% |
| crates/gitforge-review | 1006 | 5 | 99.50% |

## Reading

The deficit is concentrated in the **binary crates and the docker-bound
executor**: `services/git-server` (smart-HTTP/SSH serving lives behind
in-crate tests that spawn real `git` clients), `services/ci` (the
orchestrator main: boot order, workspace sweep, redelivery loops), and
`crates/gitforge-runner` (docker-API paths that need a daemon). The library
crates the F-series defect fixes landed in (`scheduler`, `db`, `ci`) are
already at 90–99%.

Ratchet order for the next campaign, by expected points per test-hour:

1. `services/git-server` — the two protocol suites exist in
   `gitforge-core`; driving them against the actual service binary covers
   ~550 uncovered lines with existing patterns.
2. `services/ci` — the workspace sweep, trigger redelivery, and boot-order
   paths are pure-Tokio and unit-testable; the F-series regression tests
   already exercise the query layer underneath them.
3. `crates/gitforge-runner` — executor decision paths (the four
   early-return `JobResult` grades) are testable with a sandbox trait
   double; only the bollard call sites need the daemon.

The advisory target in the gate (84%) is reachable by fixing item 1 alone
(git-server at 90% adds ≈ 3.3pp workspace-wide). A 99% workspace gate is
not a near-term target; the realistic sequence is 84 advisory → per-crate
floors (no crate below 70%) → raise both over time.

## Reproducing

```bash
# On fedora, from a clone of the commit under test:
docker run --rm --security-opt label=disable -v "$PWD":/src -w /src \
    dsc-ci-rust:10 bash -c '
      git config --global --add safe.directory "*"
      cargo llvm-cov --workspace --all-targets --summary-only \
          --fail-under-lines 82 -- --test-threads=2 > /src/cov-summary.txt 2>&1
      grep -m1 "^TOTAL" /src/cov-summary.txt'
```

`--security-opt label=disable` is required for bind-mounted host dirs under
SELinux (see RUNBOOK "Two-instance topology"). The full per-file summary
for this baseline is retained on fedora at
`/var/tmp/covprobe-main/repo/cov-summary.txt` (transient scratch — this
document is the durable record).
