# Git Hooks Documentation

GitForge uses staged git hooks plus an optional pre-commit framework layer.

## Overview

| Layer | Trigger | Purpose |
|-------|---------|---------|
| `.githooks/pre-commit` | Every `git commit` | cargo fmt --check + strict clippy when Rust sources are staged |
| `.githooks/pre-push` | Every `git push` | Full workspace test suite (2-thread bound, mirrors CI) |
| `.githooks/quality-gates` | Library | Secrets / placeholder-code / fake-data checks |
| `.pre-commit-config.yaml` | Via pre-commit framework | Repo-generic hygiene (whitespace, YAML, secrets, merge markers) |

The installed hooks live in `.githooks/`; the canonical sources live in
`scripts/hooks/` and are copied over by the installer. Edit the sources,
then re-run `make setup` (or copy both) — the installer overwrites
`.githooks/` from `scripts/hooks/`.

## Installation

```bash
make setup
# or
./scripts/install-hooks.sh
```

This sets `git config core.hooksPath .githooks`.

## Pre-commit Hook (`.githooks/pre-commit`)

Runs **before every commit**. Skips itself (fast path) when no Rust
sources or `Cargo.toml`/`Cargo.lock` are staged — docs-only commits pay
nothing.

### Checks (in order)

1. **Staged-file gate** — any staged `.rs` / `Cargo.toml` / `Cargo.lock`?
2. **cargo fmt --all -- --check** — formatting must be exact
3. **cargo clippy --workspace --all-targets -- -D warnings** — zero warnings

### Quality Gates (`.githooks/quality-gates`)

A library of checks (hardcoded secrets, bare console logging, fake
placeholder data, TODO/FIXME without issue references) that hooks can
source. It is not wired into the default pre-commit chain; wire it in by
sourcing it from `.githooks/pre-commit` if you want it locally enforced.

### Skipping Hooks

```bash
git commit --no-verify -m "WIP: temporary"
```

⚠️ Use sparingly. The authoritative gate is the GitForge pipeline
(`.gitforge.yml`); a skipped hook still has to pass CI.

## Pre-push Hook (`.githooks/pre-push`)

Runs **before every push**.

1. Full test suite: `cargo test --workspace --no-fail-fast -- --test-threads=2`

The 2-thread bound mirrors the CI test job: unbounded parallelism on this
host loses spawned-binary protocol tests to load races (the
`git_http_edges` / `ci_trigger_flow` flake class), while the bound keeps
the suite deterministic well inside CI timeouts.

## Pre-commit Framework (`.pre-commit-config.yaml`)

Runs via the pre-commit framework. Installed separately: `pre-commit install`.

### What It Checks

| Hook | Purpose |
|------|---------|
| cargo-fmt | `cargo fmt --check` on staged Rust changes |
| trailing-whitespace | No trailing whitespace |
| end-of-file-fixer | Files end with newline |
| check-yaml | YAML files are valid |
| check-added-large-files | No files > 1MB |
| check-merge-conflict | No `<<<<<<<` markers |
| check-case-conflict | No case conflicts (e.g. `File.rs` vs `file.rs`) |
| detect-private-key | No SSH private keys |
| detect-aws-credentials | No AWS credentials |
| mixed-line-ending | Report mixed line endings |

## Customizing

### Add a New Pre-commit Check

Edit `scripts/hooks/pre-commit` (then re-run `make setup`), adding after
the clippy section:

```bash
INFO "Running my new check..."
if ! my_new_check; then
    ERROR "My new check failed"
    exit 1
fi
```

Keep the staged-file gate fast: anything heavier than fmt/clippy belongs
in `pre-push` or the CI pipeline, not in per-commit paths.
