# Troubleshooting

Common issues and how to resolve them.

## Tests Failing

1. Run with the CI's thread bound: `cargo test --workspace -- --test-threads=2`
   (the 2-thread bound is deliberate — see `.gitforge.yml`)
2. Under heavy NAS load, SQLite opens can stall. Set `TMPDIR=/tmp`; if `/tmp`
   is pressured, use `/dev/shm`
3. Check git hooks are installed: `git config core.hooksPath`

## Pre-commit Hook Failures

1. Check git hooks path: `git config core.hooksPath`
2. Re-install hooks: `make setup`
3. Skip hooks temporarily: `git commit --no-verify` (use sparingly)

## Coverage Below Threshold

1. The line-coverage gate lives in `.gitforge.yml` (coverage job): **fail < 82%,
   warn < 84%**
2. Reproduce locally: `cargo llvm-cov --workspace --all-targets --summary-only
   --fail-under-lines 82 -- --test-threads=2`
3. Thresholds are calibrated against the CI sandbox, not a developer host, where
   ambient services inflate integration coverage

## Clippy or rustdoc Warnings

```bash
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

## CI Run Stuck or Failing

1. Read the run through the API: `GET /api/pipeline-runs?repo_id=<uuid>`
2. A run whose jobs all sit `queued` with no container usually means the
   scheduler could not get a database connection — the pool now has a 10 s
   acquire timeout so this degrades to visible errors instead of a silent
   wedge
3. Cancelling a superseded run: `POST /api/jobs/{id}/cancel`
4. A green run is not proof of work: confirm the durable job rows cover the
   pipeline definition before trusting it (the release gate enforces this)

## Release Gate Refuses

The gate refuses a cut unless the live instance graded the **exact** source
commit green, with a durable job row per definition job, and no skipped
quality job (clippy/test/coverage/fuzz).

- `scripts/gitforge-release-gate <40-hex-commit>` — reads run evidence
- If `sqlite3` is missing, the gate falls back to a python3 sqlite3 reader
- `GITFORGE_RELEASE_SKIP_GATE=1` skips it; the refusal to pretend is printed
  loudly and the cut is **not** gated

## Release Bundle Verification

`scripts/gitforge-release-bundle` assembles a bundle and verifies it before
publishing. The `READY` marker is covered by `MANIFEST.sha256`, so a bundle
edited after the cut fails `scripts/gitforge-release-verify`.

## Still Stuck?

- Open an issue with the `bug` label
- Include: rustc version, OS, full error output, steps to reproduce
