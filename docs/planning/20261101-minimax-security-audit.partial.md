# GitForge Security & CI-Pipeline Audit — 2026-11-01

Scope: `/nas/Temp/repos/GitForge` (Rust workspace + Python services). Method: source
reading only — no builds, no commits, no destructive git. Every finding below is
anchored to a `file:line` in the tree at commit time of the audit; anything I
could not confirm by reading code is marked **UNVERIFIED**.

Audit target: the self-hosted git forge + CI runner (`gitforge-buildd`) used by
this host. Primary concerns: CI sandbox escape surface, authz coverage on API
routes, secret handling in job specs, and panic/unwrap in production paths.

---

## 0. Architecture Map

*(pending)*

## 1. Executive Summary / Finding Table

*(pending)*

## 2. CI Runner Sandbox & Privilege Audit

*(pending)*

## 3. Command Injection in Shell Jobs

*(pending)*

## 4. Secret Handling in Job Specs

*(pending)*

## 5. API Route Authorization Audit

**Full route-by-route inventory with anchors: `docs/planning/20261101-minimax-audit-authz.md`.**
That document enumerates every route registered in
`crates/gitforge-api/src/**` with its auth class and per-handler
authorization check. Summary of the shape and the findings:

### Router construction

- `crates/gitforge-api/src/server.rs:80-153` — the full route tree. Public:
  `/health`, `/metrics`, `/swagger-ui`, `/api-docs/openapi.json`,
  `/dashboard`, and `POST /api/runners`. Auth routes (`/auth/login|refresh|
  logout|status`) are public by nature. Everything else is nested under
  `/api` behind `middleware::from_fn(crate::middleware::auth_middleware)`
  (`server.rs:140`).
- `crates/gitforge-api/src/middleware.rs:25-92` — the middleware validates
  the JWT and **re-resolves the persisted role on every request**
  (`middleware.rs:66`). It performs *authentication only*; there is no
  per-route authorization in middleware, so every handler must do its own
  check. Most do; the exceptions are F1–F3 below.

### Findings

**F1 — CRITICAL: `POST /api/runners` is fully unauthenticated and hijacks
existing runner identities.**

- Mounted as a public route (`server.rs:101-105`), outside the auth
  middleware; handler at `crates/gitforge-api/src/routes/runners.rs:92-133`
  accepts arbitrary JSON (`name`, `type`, `capacity`) and validates **no**
  token or shared secret.
- The comment at `server.rs:92-94` claims "the runner proves itself with its
  registration token inside the handler" — **no such token exists.** A grep
  for `RUNNER_REGISTRATION|registration_token|x-gitforge-runner` across
  `crates/` and `services/` returns nothing.
- Worse, `RunnerQueries::register_or_refresh`
  (`crates/gitforge-db/src/queries.rs:2505-2556`) **adopts any existing row by
  name**: posting `{"name":"<production-runner-name>","capacity":16}`
  overwrites the `runner_type`, status, capacity, labels and heartbeat of a
  live runner (`queries.rs:2532-2544`) and returns its UUID — enabling
  targeted `DELETE /api/runners/{id}` interference and, once a runner
  identity is spoofed, any downstream runner-trust assumption.
- Repro (no credential of any kind):
  `curl -X POST http://host:42780/api/runners -d '{"name":"prod-runner-1","type":"docker","capacity":64}'`

**F2 — HIGH: `POST /api/webhook/trigger/{pipeline_id}` has no repo-ownership
check.**

- `crates/gitforge-api/src/routes/webhook.rs:200` — protected router, so any
  authenticated user passes the middleware.
- Handler `trigger_pipeline` (`webhook.rs:204-344`) takes the caller only as
  `_user: AuthenticatedUser` (underscore = unused), loads the pipeline by id,
  and checks only that the payload `repo_id` matches the pipeline's own
  `repo_id` (`webhook.rs:236-249`) before delegating a real CI run
  (`client.trigger`, `webhook.rs:251-260`) or creating a run + jobs locally
  (`webhook.rs:337+`).
- No `authorize_repo` call anywhere in the handler — contrast every other
  trigger path (`ci.rs:470`, `ci.rs:635`). Any authenticated instance user
  can trigger arbitrary pipeline runs (arbitrary command execution on the CI
  runner) against any repository they do not own, by enumerating pipeline
  UUIDs.
- Repro:
  `curl -X POST http://host:42780/api/webhook/trigger/<pipeline-uuid> \
   -H "Authorization: Bearer $ANY_USER_JWT" \
   -d '{"repo_id":"<repo-uuid>","commit_hash":"<sha>","branch":"main"}'`

**F3 — MEDIUM: the runner registry is readable by any authenticated user.**

- `GET /api/runners` (`runners.rs:67-89`) and `GET /api/runners/{id}`
  (`runners.rs:136-177`) take `AuthenticatedUser` and perform no role or
  ownership check (`_user`). Even a `read_only` account can enumerate all
  runner names, types, capacities and heartbeat status — reconnaissance that
  directly feeds the F1 hijack. Retirement *is* properly gated to
  admin/maintainer (`runners.rs:187`).

**F4 — MEDIUM: the role model is instance-wide, not per-repo.**

- `can_access_repo` (`crates/gitforge-api/src/routes/repo.rs:108-110`) and
  `can_manage_jobs` (`crates/gitforge-api/src/routes/ci.rs:103-105`) grant
  **every** maintainer access to **every** repository: any maintainer can
  read, delete (`repo.rs:371`), rewrite ref policy of (`repo.rs:464`), and
  trigger runs on all users' repos. There is no repository-collaborator
  table; the check is `caller_id == owner_id || role in {admin, maintainer}`.
  If the product intends per-repo collaboration this is a design-level
  over-grant, not a one-line bug.

**F5 — LOW: unauthenticated instance telemetry.**

- `/metrics` (`server.rs:81,267-276`) and `/dashboard`
  (`crates/gitforge-api/src/dashboard.rs:232-234`) are public and expose
  Prometheus counters, repo/pipeline/run counts, success rates and
  runner-online counts with no authentication. The skip is deliberate
  (`middleware.rs:15`), but on a multi-tenant instance it leaks aggregate
  activity to anonymous parties.

### Non-findings (verified adequate)

- All CI read/write handlers (pipelines, runs, jobs, logs) route through
  `authorize_repo`/`authorized_job` with 404-collapsing for cross-tenant
  access (`ci.rs:107-149`); `list_pipeline_runs` applies the ownership filter
  *inside the SQL* via `list_scoped`
  (`crates/gitforge-db/src/queries.rs:1160-1172`) — no IDOR found.
- Artifact content download — the highest-value target — authorizes via the
  job → run → repo owner chain (`artifacts.rs:49-83`) on every route
  including content and delete; metadata existence is collapsed to 404.
- `PATCH /api/users/{id}/role` enforces admin server-side, validates role
  values, and blocks demoting the last admin (`routes/users.rs:42,98-120`).
- SSH keys are strictly caller-scoped including delete
  (`routes/ssh_keys.rs:193`, `delete_owned`).
- Auth: JWT validated per request with fresh role resolution
  (`middleware.rs:62-82`); refresh tokens are rotated on use and replay
  fails closed (`routes/auth.rs:198-303`).
- Repo names are validated against traversal/separator injection
  (`routes/repo.rs:23-72`).



## 6. Path Handling: worktrees and /var/tmp/gitforge-target

*(pending)*

## 7. unwrap() / panic! in Production Paths

**Verdict: CLEAN.** The workspace enforces the policy mechanically and the
enforcement is current.

- `Cargo.toml:125-134` — `[workspace.lints.clippy]` denies
  `unwrap_used`, `expect_used`, and `panic` workspace-wide
  (`uninlined_format_args`, `map_unwrap_or`,
  `redundant_closure_for_method_calls`, `ignored_unit_patterns` also
  denied).
- `clippy.toml` — `allow-unwrap-in-tests` / `allow-expect-in-tests` /
  `allow-panic-in-tests = true`, so the ~3200 test-context sites
  (per `docs/planning/QUALITY_BASELINE.md`, cited in the clippy.toml
  comment) are legitimately exempt while production code keeps the denies.

Brace-aware scan of all 119 non-test `.rs` files (crate/service `src/`,
`tests/` dirs excluded, `#[cfg(test)]` blocks and single annotated
`#[cfg(test)]` fns skipped) finds **16 hits, all justified**:

| Site | Class | Verdict |
|---|---|---|
| `crates/gitforge-metrics/src/lib.rs:69` | `expect` | Justified — static metric table registered into a fresh Registry; documented invariant, `#[allow(clippy::expect_used)]` with rationale |
| `crates/gitforge-db/src/queries.rs:1383` | `unreachable!` | Retry loop returns inside the loop body; documented |
| `crates/gitforge-process/src/pool.rs:85` | `expect` | Semaphore never closed (no `Semaphore::close` call exists); documented invariant + allow |
| `crates/gitforge-review/src/lib.rs:137` | `expect` | Static scanner regex; same class as metrics |
| `crates/gitforge-scheduler/src/assigner.rs:189` | `unreachable!` | Same retry-loop shape as queries.rs:1383 |
| `services/api/src/main.rs:154,164` | `panic!` | Deliberate fail-fast startup abort for missing/empty JWT secret — "a server with no signing key must not come up at all"; `#[allow(clippy::panic)]` with doc comment. Also supports `JWT_SECRET_FILE` (mode-0600 file preferred over env, which `/proc/<pid>/environ` leaks) |
| `crates/gitforge-review/src/fix.rs:111` | `expect` | **Not real code** — the hit is inside a `format!` string: it is a generated fix *suggestion text* for the "hardcoded password" template. No execution path. |
| `crates/gitforge-testing-common/src/fixtures.rs:32,53,56,64` and `src/bin/check-route-docs.rs:23,32,43` | `panic!` | Test-support crate and a test-runner binary — test context, correctly allowed |

One minor observation (INFO): the review fix-template at
`crates/gitforge-review/src/fix.rs:111` generates
`std::env::var(...).expect(...)` as its suggested remediation — i.e. the
reviewer tool recommends code that the workspace's own clippy config
denies. Cosmetic inconsistency in suggested code, not a defect in
GitForge itself.

UNVERIFIED: none. The scan covered every production `.rs` file; the
remaining `#[allow]`-free hits would fail `cargo clippy -D warnings`,
which is the repo's own CI gate.



## 8. Open Questions for the Operator

*(pending)*
