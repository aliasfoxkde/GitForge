# API Authorization Audit — 2026-11-01

Scope: HTTP API surface only — `crates/gitforge-api/src/**` and `services/api/src/**`.
Method: source read of every route registration and handler. No builds, no file
modifications outside this report.

## Router construction (evidence anchors)

- `crates/gitforge-api/src/server.rs:80-153` — full route tree:
  - Public (no auth): `/health`, `/metrics`, `/swagger-ui`, `/api-docs/openapi.json`,
    `/dashboard`, and `POST /api/runners` (runner registration, rate-limited).
  - Auth routes (public): `/auth/login|refresh|logout|status` (server.rs:116-123).
  - Protected: everything else, nested under `/api` behind
    `middleware::from_fn(crate::middleware::auth_middleware)` (server.rs:140).
- `crates/gitforge-api/src/middleware.rs:25-92` — auth middleware validates the JWT
  and re-resolves the persisted role on every request (middleware.rs:66). It performs
  **authentication only**; there is no per-route authorization in middleware.
- `services/api/src/main.rs:65-71` — production wiring adds storage + CI trigger
  client; no additional routers.

## Route inventory

Legend: AU = any authenticated user; role names are instance-wide roles.

| Method | Path | Auth | Handler authz check | Anchor |
|---|---|---|---|---|
| GET | /health | none | none (by design) | server.rs:80,236 |
| GET | /metrics | none | none — see F5 | server.rs:81 |
| GET | /dashboard | none | none — see F5 | dashboard.rs:232-234 |
| GET | /swagger-ui, /api-docs/openapi.json | none | none (by design) | server.rs:82-84 |
| POST | /api/runners | **none** | **NONE — see F1** | runners.rs:63,92-133 |
| POST | /auth/login | none | password verify | routes/auth.rs:96 |
| POST | /auth/refresh | none | refresh-digest lookup + rotation | routes/auth.rs:198 |
| POST | /auth/logout | none | digest revocation (idempotent) | routes/auth.rs:308 |
| GET | /auth/status | optional | validates presented token | routes/auth.rs:325 |
| GET | /api/repos | AU | owner or admin/maintainer filter (repo.rs:108-110,184) | repo.rs:96 |
| POST | /api/repos | AU | owner = caller (repo.rs:217) | repo.rs:96 |
| GET | /api/repos/{id} | AU | owner or admin/maintainer (repo.rs:316) | repo.rs:97 |
| DELETE | /api/repos/{id} | AU | owner or admin/maintainer (repo.rs:371) | repo.rs:97 |
| GET/PATCH | /api/repos/{owner}/{repo}/policy | AU | owner or admin/maintainer via resolve_policy_repo (repo.rs:129-131) | repo.rs:98-101 |
| GET | /api/repos/{owner}/{repo}/commits/{sha}/status | AU | same (repo.rs:578) | repo.rs:102-105 |
| GET | /api/pipelines | AU | per-row authorize_repo (ci.rs:163) | ci.rs:90 |
| POST | /api/pipelines | AU | authorize_repo on resolved repo (ci.rs:338) | ci.rs:90 |
| GET/DELETE | /api/pipelines/{id} | AU | authorize_repo (ci.rs:203, 801) | ci.rs:91 |
| POST | /api/pipelines/{id}/runs | AU | authorize_repo (ci.rs:470) | ci.rs:92 |
| GET | /api/pipeline-runs | AU | SQL-scoped: non-managers get owner_id filter (ci.rs:922-928; queries.rs:1152-1191) | ci.rs:93 |
| GET | /api/pipeline-runs/{id} | AU | authorize_repo (ci.rs:974) | ci.rs:94 |
| POST | /api/pipeline-runs/{id}/rerun | AU | authorize_repo (ci.rs:635) | ci.rs:95 |
| GET | /api/pipeline-runs/{id}/jobs | AU | authorize_repo (ci.rs:1057) | ci.rs:96 |
| POST | /api/jobs | AU | authorize_repo via run (ci.rs:1176) | ci.rs:100 |
| GET | /api/jobs/{id} | AU | authorized_job → authorize_repo (ci.rs:128-149, 1335) | ci.rs:97 |
| GET | /api/jobs/{id}/logs | AU | authorized_job (ci.rs:1379) | ci.rs:98 |
| POST | /api/jobs/{id}/cancel | AU | authorized_job (ci.rs:1440) | ci.rs:99 |
| GET | /api/artifacts | AU | per-artifact authorize_job (artifacts.rs:109) | artifacts.rs:38 |
| GET/DELETE | /api/artifacts/{id} | AU | authorize_job (artifacts.rs:165, 289) | artifacts.rs:39 |
| GET | /api/artifacts/{id}/content | AU | authorize_job (artifacts.rs:225) | artifacts.rs:40 |
| GET | /api/jobs/{job_id}/artifacts | AU | authorize_job (artifacts.rs:338) | artifacts.rs:41 |
| GET | /api/runners | AU | **NONE** — see F3 | runners.rs:56,67-89 |
| GET | /api/runners/{id} | AU | **NONE** — see F3 | runners.rs:57,136-177 |
| DELETE | /api/runners/{id} | AU | admin/maintainer role (runners.rs:187) | runners.rs:57 |
| PATCH | /api/users/{id}/role | AU | admin role + last-admin guard (users.rs:42,98) | users.rs:29 |
| POST | /api/ssh-keys | AU | owner = caller (ssh_keys.rs:133) | ssh_keys.rs:56 |
| GET | /api/ssh-keys | AU | scoped to caller (ssh_keys.rs:160) | ssh_keys.rs:56 |
| DELETE | /api/ssh-keys/{id} | AU | delete_owned(key_id, caller) (ssh_keys.rs:193) | ssh_keys.rs:57 |
| POST | /api/webhook/trigger/{pipeline_id} | AU | **NONE — see F2** | webhook.rs:200,204 |
| POST | /api/review-runs | AU | authorize_repo (review.rs:294) | review.rs:69 |
| GET | /api/review-runs/{id} | AU | authorized_review_run (review.rs:265, 369) | review.rs:70 |
| GET | /api/review-runs/{id}/findings | AU | authorized_review_run (review.rs:402) | review.rs:71 |

## Findings (most severe first)

### F1 — CRITICAL: `POST /api/runners` is fully unauthenticated and hijacks existing runner identities

- Registration: `crates/gitforge-api/src/routes/runners.rs:63` mounted as a public
  route at `server.rs:101-105` (deliberately outside the auth middleware).
- The handler (`runners.rs:92-133`) accepts arbitrary JSON (`name`, `type`,
  `capacity`) and never validates any token or shared secret. The code comment at
  `server.rs:92-94` claims "the runner proves itself with its registration token
  inside the handler" — **no such token exists**. `grep` for
  `RUNNER_REGISTRATION|registration_token|x-gitforge-runner` across crates/ and
  services/ returns nothing.
- Worse, `RunnerQueries::register_or_refresh` (`crates/gitforge-db/src/queries.rs:2505-2556`)
  adopts any existing row by name: an attacker who POSTs
  `{"name":"<production-runner-name>","capacity":16}` **overwrites the runner_type,
  status, capacity, labels and heartbeat of a live runner**
  (queries.rs:2532-2544) and receives its UUID, enabling subsequent targeted
  `DELETE /api/runners/{id}` interference and, once a runner identity is spoofed,
  any runner-trust downstream assumptions.
- Repro (no token at all):
  `curl -X POST http://host:42780/api/runners -d '{"name":"prod-runner-1","type":"docker","capacity":64}'`

### F2 — HIGH: `POST /api/webhook/trigger/{pipeline_id}` has no repo-ownership check

- Registration: `webhook.rs:200` (protected router, so any authenticated user passes).
- Handler `trigger_pipeline` (`webhook.rs:204-344+`) extracts the caller only as
  `_user: AuthenticatedUser` (underscore — unused), loads the pipeline by id, checks
  only that the payload `repo_id` matches the pipeline's own `repo_id`
  (webhook.rs:236-249), then delegates a real CI run (`client.trigger`,
  webhook.rs:251-260) or creates a run + jobs locally (webhook.rs:337+).
- No `authorize_repo` call anywhere in the handler (contrast with every other
  trigger path: ci.rs:470, ci.rs:635). Any authenticated user of the instance can
  trigger arbitrary pipeline runs (arbitrary command execution on the CI runner)
  against any repository they do not own, by guessing or enumerating a pipeline UUID.
- Repro: `curl -X POST http://host:42780/api/webhook/trigger/<pipeline-uuid> \
  -H "Authorization: Bearer $ANY_USER_JWT" -d '{"repo_id":"<repo-uuid>","commit_hash":"<sha>","branch":"main"}'`

### F3 — MEDIUM: runner registry readable by any authenticated user

- `GET /api/runners` (runners.rs:67-89) and `GET /api/runners/{id}`
  (runners.rs:136-177) take `AuthenticatedUser` and perform no role or ownership
  check (`_user`). Any account — including `read_only` — can enumerate all runner
  names, types, capacities and heartbeat status. This is an information-disclosure /
  reconnaissance gap (runner names feed the F1 hijack). Retirement is properly
  gated to admin/maintainer (runners.rs:187).

### F4 — MEDIUM: role model is instance-wide, not per-repo

- `can_access_repo` (`repo.rs:108-110`) and `can_manage_jobs` (`ci.rs:103-105`)
  grant **every** maintainer access to **every** repository: any maintainer can
  read, delete (`repo.rs:371`), rewrite ref policy of (`repo.rs:464`), and trigger
  runs on all users' repos. There is no repository-collaborator table; the check is
  `caller_id == owner_id || role in {admin, maintainer}`. If the product intends
  per-repo collaboration this is a design-level over-grant, not a one-line bug.

### F5 — LOW: unauthenticated instance telemetry

- `/metrics` (`server.rs:81,267-276`) and `/dashboard` (`dashboard.rs:232-234`)
  are public and expose Prometheus counters, repo/pipeline/run counts, success
  rates and runner-online counts with no authentication. Comment at
  middleware.rs:15 shows the skip is intentional, but on a multi-tenant instance
  this leaks aggregate activity to anonymous parties.

### Non-findings (verified adequate)

- All CI read/write handlers (pipelines, runs, jobs, logs) route through
  `authorize_repo`/`authorized_job` with 404-collapsing for cross-tenant access
  (ci.rs:107-149); `list_pipeline_runs` applies the ownership filter inside the SQL
  via `list_scoped` (queries.rs:1160-1172) — no IDOR found.
- Artifact content download, the highest-value target, authorizes via
  job → run → repo owner chain (artifacts.rs:49-83) on every route including
  content and delete; metadata-existence is collapsed to 404.
- Admin route `PATCH /api/users/{id}/role` enforces admin role server-side,
  validates role values, and blocks demoting the last admin (users.rs:42,98-120).
- SSH keys are strictly caller-scoped including delete (ssh_keys.rs:193,
  `delete_owned`).
- Auth: JWT validated per request with fresh role resolution
  (middleware.rs:62-82); refresh tokens are rotated on use and replay fails
  closed (routes/auth.rs:198-303).
- Repo names validated against traversal/separator injection (repo.rs:23-72).

## UNVERIFIED

- None within the stated scope. Every route in the four route modules
  (repo, ci, runners+public, users, ssh_keys, artifacts, webhook, review, auth,
  dashboard) was read in full. The CI service (port 42781) and git-server (42782)
  surfaces are out of scope and were not audited; note that F2's impact depends on
  the CI trigger endpoint trusting the loopback token, which was not verified.

## Priority fixes

1. F1: require a runner registration token (e.g. `GITFORGE_RUNNER_REGISTRATION_TOKEN`,
   HMAC over the payload) in `register_runner`; forbid silent name adoption or
   make it prove possession of the prior runner's secret.
2. F2: add `authorize_repo(&pool, &claims, pipeline.repo_id)` before the trigger
   delegation in `trigger_pipeline` (webhook.rs, after the existence check at 234).
3. F3: gate `list_runners`/`get_runner` behind `can_manage_jobs`.

## Dispositions (2026-11-03)

- **F1 — RESOLVED**: runner registration requires a token
  (`GITFORGE_RUNNER_REGISTRATION_TOKEN`); enforced in `register_runner`.
  Commit `338a43b1` / PR #365.
- **F2 — RESOLVED**: `trigger_pipeline` authorizes the pipeline's repo
  before delegating to the CI service. Commit `338a43b1` / PR #365.
- **F3 — RESOLVED**: `GET /api/runners` and `GET /api/runners/{id}` are
  gated to maintainer/admin via `can_manage_jobs`. Commits `607667e1`,
  `45547d40` (branch `integrate/audit-fixes-20261102`).
- **F4 — WONTFIX (design decision, recorded)**: the instance-wide role
  model (owner or admin/maintainer, no per-repo collaborator table) is
  retained deliberately. No product document calls for per-repo
  collaboration; the deployment model is a single-operator instance
  (RUNBOOK), and the maintainers-on-every-repo semantic matches the
  platform's stated trust boundary. If a multi-tenant offering is ever
  scoped, this becomes the first design item — introduce a
  `repo_collaborators` table and thread it through `can_access_repo` /
  `authorize_repo` / `can_manage_jobs`.
- **F5 — RESOLVED**: `/metrics` and `/dashboard` now require
  authentication unless `GITFORGE_PUBLIC_TELEMETRY=1` is set
  (`middleware.rs`); `/health` stays public (liveness probe, no data).
  Decision core `telemetry_is_public` is unit-tested including the
  middleware branch. This changes the default for existing deployments:
  set the env var to keep scrape access after upgrade.
