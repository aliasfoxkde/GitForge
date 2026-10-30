# Coverage Guide

Measuring and maintaining line coverage for the GitForge workspace.

## The Gate

The line-coverage gate lives in `.gitforge.yml` (coverage job):

- **fail < 82%**
- **warn < 84%**

Thresholds are calibrated against the CI sandbox (`dsc-ci-rust`), not a
developer host, where ambient services inflate integration coverage. Raise
them only with an in-sandbox measurement.

## Measuring Locally

```bash
cargo llvm-cov --workspace --all-targets --summary-only \
    --fail-under-lines 82 -- --test-threads=2
```

The `--test-threads=2` bound mirrors the CI suite (some suites flake at higher
parallelism under co-tenant load).

For an HTML report:

```bash
cargo llvm-cov --workspace --all-targets --html -- --test-threads=2
# open target/llvm-cov/html/index.html
```

## Conventions

- Unit tests live in `#[cfg(test)]` modules; integration tests in
  `crates/*/tests/` and `services/*/tests/`
- Regression-test every defect with a pinned test
- Tests that need temp files should honor `TMPDIR` and keep payloads small —
  SQLite test stalls under NAS-backed temp dirs are a known failure mode
- Prefer generated inputs over credential-like literals in tests; secrets
  scanners and CodeQL flag hard-coded passphrases even in test code

## Historical Baseline

- 83.04% at `c4e3816b` (2026-10-10, in-sandbox measurement,
  `docs/audits/COVERAGE_BASELINE_2026-10-10.md`)
