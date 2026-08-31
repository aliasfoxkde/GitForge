# GitForge agent guidance

## Execution boundary

GitForge is the remote Git, CI, runner, artifact, and release plane. Keep
heavy builds, workspace tests, browser tests, coverage, and release assembly on
the Fedora runner when a GitForge pipeline can execute them. Keep jobs bounded
by their declared timeout and resource policy; never start unbounded parallel
workspace workloads on the coordinator.

## Change workflow

1. Inspect the current branch, worktree, manifest, and relevant service or
   workflow before editing.
2. Make changes on a focused branch and preserve the existing release/data
   separation and rollback pointer.
3. Run `cargo fmt --all -- --check`, focused tests, and the repository's
   security/quality gates. Use serialized workspace tests where shared build
   state can race.
4. Require hosted CI, Aegis, security checks, and a clean merge state before
   promotion. Build an immutable release bundle, verify checksums, atomically
   switch only `gitforge-current`, restart the four managed services once, and
   verify `/health` plus service state.
5. Record observable results, commit, push, and retain the previous release for
   rollback. A green transport response alone is not semantic validation.

## Safety invariants

- Reject malformed pipeline DAGs, missing dependencies, duplicate job names,
  invalid timeouts, and stale or conflicting leases fail-closed.
- Treat job commands and images as trusted repository configuration only after
  validation; do not broaden command execution privileges from a test.
- Keep secrets in service-managed environment files and never commit or print
  credentials.
- Do not modify Amortyx from this repository; consume its released contract.

Project-specific workflow details live in the repository manifests, scripts,
and `.github` workflows; consult those sources before copying commands here.
