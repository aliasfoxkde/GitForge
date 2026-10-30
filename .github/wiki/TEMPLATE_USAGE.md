# Release & Deployment Usage

How GitForge releases are cut, verified, and deployed.

## Release Sequence

1. **Gate** — `scripts/gitforge-release-gate <commit>` refuses a cut unless the
   live instance graded the exact source commit green with full durable job
   coverage (and no skipped quality jobs)
2. **Bundle** — `scripts/gitforge-release-bundle` assembles the release bundle
   and self-verifies before publishing; `READY` is covered by
   `MANIFEST.sha256`
3. **Promote** — flip the `gitforge-current` symlink to the new bundle
4. **Restart** — drain-gated cutover (`scripts/gitforge-remote-cutover`):
   stop schedulers first, then stateless units; start in reverse
5. **Tag** — GitHub tag + release pushed GitForge-first, then mirrored

## Gate Evidence Requirements

- A `succeeded` pipeline run for the **exact** commit
- A durable row per pipeline-definition job (the lazy-enqueue false green left
  green rows that never executed)
- No skipped quality job (`clippy`, `test`, `coverage`, `fuzz`) unless
  `GITFORGE_RELEASE_ALLOW_SKIPPED=1` for a genuinely docs-only cut
- The gate reads evidence via the `sqlite3` CLI, or via a python3 fallback
  when the CLI is absent

## Cut Etiquette

- Check for co-tenant cuts: dry-run the promote and read `previous=` before
  publishing — absorb their branch rather than racing
- A ruleset bypass may be needed to merge to GitHub main (Safeguards ruleset);
  use `gh pr merge --admin` with a temporary bypass grant
- The runner image (`dsc-ci-rust:N`) must be rebuilt whenever the lockfile
  changes; "candidate versions didn't match" errors mean a stale image cache
