# GitForge Runner Config CI Fix — Implementation Receipt

**Receipt date**: 2026-08-29
**Status**: to be verified by coordinator

---

## Parent Commit

`6045a28` — docs: correct validation receipt ref

---

## Code Commit

`ab4d6e4` — fix(gitforge-runner): eliminate field-reassign-with-default clippy warning and fix parallel test races

---

## Changed Files

| File | Change |
|------|--------|
| `crates/gitforge-runner/src/agent.rs` | 1) Replaced `let mut cfg = Self::default()` + field reassignments in `from_env()` with a pure `parse_from_iter` private helper that collects vars into locals then constructs `Self` directly — eliminates `clippy::field-reassign-with-default` warning. 2) `from_env()` now delegates to `parse_from_iter(std::env::vars())`. 3) Replaced all `temp_env::with_vars()` global-env-mutating tests with parallel-safe tests calling `parse_from_iter` directly with pure `Vec<(String, String)>` input — no `std::env::set_var`/`remove_var`, no races. 4) Added 3 new tests: whitespace-only values, unknown-key ignored, `Default` sanity. 5) Added `err_contains` helper for deterministic variable-specific error assertions. 6) Removed the `temp_env` module. Total: 60 tests pass. |

---

## Problem 1: `clippy::field-reassign-with-default`

**Symptom**: `cargo clippy -p gitforge-runner --lib -- -D warnings` fails with
`clippy::field-reassign-with-default` at `crates/gitforge-runner/src/agent.rs:108` because
`from_env` started with `let mut cfg = Self::default()` then assigned every field.

**Fix**: Introduced `RunnerConfig::parse_from_iter<I, K, V>(iter: I) -> Result<Self>` where
`I: IntoIterator<Item = (K, V)>` with `K: AsRef<str>`, `V: AsRef<str>`. The function
collects environment variables into local `Option` fields during a single-pass iteration,
then constructs `Self { ... }` at the end with no field reassignments. `from_env()` becomes
a one-liner: `Self::parse_from_iter(std::env::vars())`.

---

## Problem 2: Parallel Test Races

**Symptom**: Three tests (`invalid_capacity`, `invalid_heartbeat_interval`, `invalid_fetch_interval`)
fail under `cargo test --workspace --no-fail-fast` (parallel) but pass with `--test-threads=1`.
The `temp_env::with_vars` module called `std::env::set_var`/`remove_var` on the **process-global**
environment, causing races when tests ran concurrently.

**Fix**: All configuration tests now call `parse_from_iter` directly with a pure `Vec<(String, String)>`
constructed from test-local `env()` helpers. No global process environment is touched, making tests
inherently safe under parallel execution. Coverage preserved: missing, empty, whitespace-only,
invalid parse, zero, negative, valid, defaults, unknown keys, and `Default()`.

---

## Validation Commands

```bash
# 1. Formatting check
cargo fmt --all --check
# exit 0

# 2. Parallel lib tests (default threads — no --test-threads=1)
TMPDIR=/home/mkinney/.cache cargo test -p gitforge-runner --lib
# result: 60 passed; 0 failed; exit 0

# 3. Clippy with -D warnings (workspace default)
cargo clippy -p gitforge-runner --lib -- -D warnings
# exit 0 (no field-reassign-with-default warning)

# 4. Verify no field-reassign-with-default specifically
cargo clippy -p gitforge-runner --lib -- -W clippy::field-reassign-with-default
# exit 0 (warning absent)
```

---

## Test Count Summary

| Category | Count |
|----------|-------|
| Config parse tests (`config_tests`) | 14 |
| `agent` unit tests | 27 |
| `executor` unit tests | 9 |
| `tests` module (additional) | 10 |
| **Total** | **60** |

All 60 tests pass in parallel mode (no `--test-threads=1`).

---

## Limitations

- The `parse_from_iter` helper is `fn` (not `async`), which is correct — parsing is CPU-bound.
- `from_env()` now calls `std::env::vars()` on every invocation; this is the same behavior as before.
- The CI `test` job (`cargo test --workspace --locked --no-fail-fast`) runs the full workspace;
  if other crates have parallel test races unrelated to `gitforge-runner`, they are outside this fix's scope.
- The `test-serialized` job in `rust-ci.yml` (`--test-threads=1`) remains as a belt-and-suspenders
  safeguard for the broader workspace, but is no longer required for the `gitforge-runner` crate.
