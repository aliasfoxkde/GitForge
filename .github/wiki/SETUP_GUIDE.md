# Setup Guide

Deploying and configuring a GitForge instance.

## Prerequisites

- Rust (stable) toolchain for building the release binaries
- Linux host with systemd (user units are the supported layout)
- SQLite database reachable by all services
- A container runtime (docker or podman) for CI job execution

## Build

```bash
make build-release
# Binaries: target/release/{api,ci,git-server,runner}
```

Note the package names are `api`, `ci`, `git-server`, `runner` (not
`gitforge-*`).

For a shared build cache across worktrees/experiments:

```bash
CARGO_TARGET_DIR=/path/to/shared/target make build-release
```

## Service Units (user layout)

The standalone-instance layout runs plain user services:

| Unit | Executes |
|---|---|
| `gitforge-api.service` | the api gateway |
| `gitforge-ci.service` | the CI scheduler |
| `gitforge-git-server.service` | git HTTP + SSH serving |
| `gitforge-runner.service` | the container runner |

Releases are staged as bundles under a `releases/` directory and a
`gitforge-current` symlink selects the live one. Cutover is:
drain in-flight jobs → stop schedulers (`ci`, `runner`) first → stop stateless
(`api`, `git-server`) → flip the symlink atomically → start stateless → start
schedulers. `scripts/gitforge-remote-cutover` automates this (it requires the
`sqlite3` CLI for its drain gate; hand-verify the drain and run the steps
manually where the CLI is absent).

## Environment

Key variables (set per-unit via drop-ins):

| Variable | Purpose |
|---|---|
| `GITFORGE_DATABASE_URL` | `sqlite:` URL of the audit database |
| `GITFORGE_CI_TRIGGER_TOKEN` | credential for `/pipelines/trigger` (401s without it) |
| `PORT` | api listen port (default 42780) |

## CI Pipeline Configuration

Pipelines are defined per-repository in `.gitforge.yml`:

```yaml
jobs:
  - name: clippy
    image: dsc-ci-rust:12
    steps:
      - name: lint
        run: cargo clippy --workspace --all-targets -- -D warnings
```

- The parser **rejects unknown top-level keys** (`cache:`, `skip_if:` on an
  old binary were silently dropped — check the executing binary's commit
  before trusting a run)
- Push triggers authenticate with `GITFORGE_CI_TRIGGER_TOKEN`
- A `202` with `pipeline_run_id: null` is the durable-queued contract: the run
  row lands late (observed up to 5 min under load), never lost

## GitHub Mirror Bridge

The repository mirrors to GitHub; CI is authoritative on the GitForge side
(`.github/workflows/gitforge-ci.yml` enqueues builds via the scheduler API).
Repository variables/secrets:

| Name | Kind | Purpose |
|---|---|---|
| `GITFORGE_ENABLED` | var | gate for the bridge workflows |
| `GITFORGE_SCHEDULER_URL` | var | scheduler endpoint |
| `GITFORGE_REPO_ID` | var | GitForge repo **UUID** (not the GitHub repo id) |
| `GITFORGE_CI_TRIGGER_TOKEN` | secret | enqueue credential |
| `GITFORGE_API_TOKEN` | secret | status-polling credential |

## Verification

```bash
make lint
make test
curl -s -o /dev/null -w '%{http_code}\n' http://localhost:42780/api/health
```

## Getting Help

- [Troubleshooting Guide](TROUBLESHOOTING)
- `docs/RUNBOOK.md` in the repository for live-instance operations
