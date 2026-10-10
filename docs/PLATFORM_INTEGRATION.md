# Platform Integration: GitForge

**Component:** GitForge CI/CD Platform
**Canonical Repo:** `/nas/Temp/repos/GitForge`
**Integration Version:** 1.0
**Created:** 2026-08-09
**Status:** Active

---

## Role

GitForge is a **Layer 2 (Execution)** component providing self-hosted Git hosting and event-driven CI/CD pipeline orchestration. It is the "How to build/test?" runtime — receives Git events from Harness, schedules CI jobs, and coordinates runner agents for isolated Docker-based job execution.

## Ownership Boundary

| Owns | Does Not Own |
|------|-------------|
| Git SSH/HTTP hosting | Which commits to build (Harness/Git trigger) |
| Pipeline orchestration | Verification policy (Oracle) |
| Job scheduling | Code execution itself (Sandbox/Runner) |
| Artifact storage | AI routing (Amortyx) |

---

## Startup Command

```bash
# Build all services
cargo build --release

# Start individual services
./target/release/api &
./target/release/ci &
./target/release/git-server &
./target/release/runner &
```

---

## Services and Ports

| Service | Binary | Default Port | Env Var | Purpose |
|---------|--------|-------------|---------|---------|
| API Gateway | `api` | 42780 | `PORT` | REST API server |
| CI Orchestrator | `ci` | 42781 | `SCHEDULER_PORT` | Pipeline scheduler |
| Git HTTP Server | `git-server` | 42782 | `HTTP_PORT` | Git over HTTP |
| Git SSH Server | `git-server` | 42022 | `SSH_PORT` | Git over SSH |
| Runner Agent | `runner` | dynamic | — | Job execution (Docker) |

---

## Health Check

```bash
# Health endpoints unknown — not confirmed in README or ARCHITECTURE.md
curl http://127.0.0.1:42780/health   # API — unknown
curl http://127.0.0.1:42781/health   # CI — unknown
```

---

## API Surface

### API Gateway (port 42780)

| Endpoint | Method | Purpose |
|----------|--------|---------|
| TBD | — | Full API reference in `docs/API.md` |

### CI Orchestrator (port 42781)

| Endpoint | Method | Purpose |
|----------|--------|---------|
| TBD | — | Scheduler/pipeline API |

### Outbound APIs

| Component | Purpose |
|-----------|---------|
| Oracle | Verification integration (`oracle-gitforge` crate exists) |
| Sandbox/Runner | Job execution |

---

## Depends On

- **Docker** — job isolation
- **Oracle** — TBD verification integration
- **Sandbox** — TBD job execution integration

---

## Required Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `42780` | API gateway port |
| `SCHEDULER_PORT` | `42781` | CI scheduler port |
| `HTTP_PORT` | `42782` | Git HTTP server port |
| `SSH_PORT` | `42022` | Git SSH server port |
| `DATABASE_URL` | — | Postgres connection string (TBD) |
| `RUST_LOG` | `info` | Log level |

---

## Platform Configuration (config/services.yaml)

```yaml
- name: gitforge
  repo: GitForge
  working_dir: /nas/Temp/repos/GitForge
  port: 42780
  command: ./target/release/api &
  health: http://127.0.0.1:42780/health  # TBD — unconfirmed
  depends_on: []

- name: gitforge-ci
  port: 42781
  command: ./target/release/ci &
  health: http://127.0.0.1:42781/health  # TBD — unconfirmed

- name: gitforge-git-server
  ports: [42782, 42022]
  command: ./target/release/git-server &
```

---

## Current Gaps

- [ ] Health check endpoints not confirmed for any service
- [ ] Full REST API endpoint list not extracted
- [ ] Runner agent port assignment mechanism undocumented
- [ ] `docs/API.md` not reviewed for endpoint details
- [ ] `oracle-gitforge` integration scope undocumented
- [ ] Prometheus metrics endpoint unknown
