# Project bootstrap with gitforge-testing-common

The shared quality bar for every new project in this fleet: 99% line
coverage, strict clippy, Aegis-clean source, no doc drift. The helpers
that enforce it live in `crates/gitforge-testing-common`.

## New-library checklist

```bash
cargo new --lib my-new-project
cd my-new-project

# 1. Test helpers, as dev-dependency:
cat >> Cargo.toml <<'TOML'

[dev-dependencies]
gitforge-testing-common = { path = "../gitforge-testing-common" }
TOML

# 2. Quality lints (strict clippy, no unwrap in production surface):
cat >> Cargo.toml <<'TOML'

[lints.rust]
rust_2018_idioms = "warn"

[lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
dbg_macro = "warn"
TOML
```

## Makefile template

Drop this `Makefile` in the project root — it mirrors the GitForge CI
chain (`fmt → clippy → test → coverage → aegis`) so a local run IS the
pipeline:

```make
# Quality gates. Mirrors .gitforge.yml — keep both in step.
COVERAGE_MIN := 99

.PHONY: lint test coverage aegis check

lint:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

test:
	cargo test --workspace

coverage: ## line-coverage gate: fail below $(COVERAGE_MIN)%
	cargo llvm-cov --workspace --all-targets --summary-only \
	    --fail-under-lines $(COVERAGE_MIN)

aegis:
	aegis --format json scan . | aegis triage --baseline .github/aegis-baseline.json

check: lint test coverage aegis
```

## Route-docs drift gate

Every axum route must exist in the OpenAPI spec AND `docs/API.md`, in
both directions. Run the packaged checker against any checkout:

```bash
cargo run -p gitforge-testing-common --bin check-route-docs -- /path/to/repo
# or: GITFORGE_REPO_ROOT=/path/to/repo check-route-docs
```

## What the crate provides

| Module | Use it for |
|---|---|
| `generators::Seed` | deterministic names/uuids/timestamps — failing tests replay |
| `fixtures::TempDir` | self-cleaning temp roots that honor `TMPDIR` and survive clones |
| `asserts::*` | `assert_contains` / `assert_not_contains` / `assert_set_eq` with readable failure output |
| `bin/check-route-docs` | the route-vs-docs drift gate, packaged |
