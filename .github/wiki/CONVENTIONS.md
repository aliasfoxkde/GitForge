# Conventions

Coding and collaboration conventions for the GitForge repository.

## Conventional Commits

Format: `<type>(<scope>): <subject>` — subject max 50 chars, imperative; body
wrapped at 72 chars.

| Type | Description |
|------|-------------|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation only |
| `test` | Adding or updating tests |
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `ci` | CI/CD changes |
| `chore` | Maintenance tasks |
| `perf` | Performance improvements |
| `style` | Formatting (no code change) |

## Rust Standards

- No `unwrap()`/`expect()` in library code — return `Result`; `?` over panics
- Structured logging via `tracing`, never `println!`/`eprintln!`
- No global mutable state — pass pools/registries explicitly
- No placeholder code: `TODO`/`FIXME`/stubs left in place
- `cargo clippy --workspace --all-targets -- -D warnings` must pass
- `cargo fmt --all` must pass

## Database Discipline

- Write paths use `BEGIN IMMEDIATE` transactions (`begin_with`)
- One-shot DB writes under contention are the F21/F23 failure class: new write
  paths go through `persist_with_retry` or a conditional/idempotent UPDATE
- Never migrate the schema while a peer binary is running against the same DB

## Testing

- Unit tests in `#[cfg(test)]` modules; integration tests in `crates/*/tests/`
  and `services/*/tests/`
- Regression-test every defect with a pinned test
- The pre-push hook runs the full suite; the CI test job runs with
  `--test-threads=2` deliberately

## CI Notes

- The authoritative CI chain is this repo's own GitForge pipeline
  (`.gitforge.yml`, self-hosted): fmt → clippy (+ rustdoc gate) → test →
  coverage on `dsc-ci-rust`
- GitHub Actions is a mirror, not the gate — validate through GitForge
- A `skipped` quality job (clippy/test/coverage/fuzz) counts as **not
  validated** for release purposes; the release gate refuses such runs

## Decision Cores Take Their Environment as Data

A decision function must not read process-global state (env vars, cwd, the
clock) *inside* the decision. Read it at the edge, once, and pass it in as a
value — concurrent writers make environment-reading cores nondeterministic.
`fn decide(input)` is the edge; `fn decide_with(input, env_value)` is the core
tests exercise.
