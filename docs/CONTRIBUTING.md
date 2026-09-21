# Contributing to GitForge

Thank you for your interest in contributing! GitForge is a Rust
workspace: fifteen libraries under `crates/` and four service binaries
under `services/`.

## Branch Strategy

```
main (production)
  └── feature/description, fix/description, docs/description, ...
```

Work happens on short-lived branches cut from `main` and lands through
pull requests. See [BRANCH_STRATEGY.md](BRANCH_STRATEGY.md).

## Commit Convention

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add new feature
fix: fix a bug
docs: documentation only
test: adding or updating tests
refactor: code refactoring (no behavior change)
ci: CI/CD changes
chore: maintenance tasks
perf: performance improvements
```

**Fix format:** `fix: correct X — was doing Y instead of Z`

## Pull Request Checklist

- [ ] Conventional commit format in title
- [ ] Root cause analysis in PR description (for `fix:` prefixes)
- [ ] `make lint` passes — formatting, `go vet`-equivalent checks, and
      `cargo clippy --workspace --all-targets -- -D warnings` (which
      also enforces the workspace's promoted pedantic lints)
- [ ] `make test` passes (`cargo test --workspace`; the pre-push hook
      additionally runs the coverage gate)
- [ ] Coverage maintained or improved (`make coverage`)
- [ ] `make vuln` clean (`cargo audit` with the shared ignore list in
      `.cargo/audit.toml`)
- [ ] ADR added/updated in `docs/architecture/` for architectural
      decisions

## Coding Standards

- **No `unwrap()` in library code** — use `?` or explicit error
  handling; `Response`-style builders degrade gracefully instead of
  panicking (see `services/git-server/src/main.rs::finish_response`)
- **Structured logging** — `tracing`, not `println!`
- **Context propagation** — async public APIs take `&Pool`/state via
  extractors and return `Result` with typed errors
- **Sentinel errors** — `thiserror` error enums per crate
- **Tests** — write tests alongside code, not deferred; route behavior
  is covered by integration tests driving the real router
  (`crates/gitforge-api/tests/`)
- **Workspace lints** — `[workspace.lints.clippy]` in the root
  `Cargo.toml` promotes selected pedantic lints to `deny`; every crate
  opts in via `[lints] workspace = true`

## Coverage Requirements

| Component | Minimum |
|-----------|---------|
| Core business logic | 95% |
| API handlers | 90% |
| Configuration | 85% |
| Utilities | 85% |

Run `make coverage` for the current numbers; per-crate coverage notes
live in `docs/planning/IMPROVEMENTS.md`.

## Documentation

Every exported item must have a doc comment explaining what it does,
its inputs and outputs, and error conditions. Architectural decisions
get an ADR in `docs/architecture/`. Wire-format behavior (JSON field
names, renames) must be pinned by a test, not just documented.

## Security

- Never commit secrets, credentials, or API keys
- Use environment variables for all configuration
- Report security issues via GitHub Advisories, not public issues
