# Aegis Repo-Registration Discrepancy — Investigation Handoff

- **Date:** 2026-10-11
- **Author:** planning/research agent (GLM-5-Turbo session), on behalf of operator
- **Status:** handoff for review — **DRAFT PR, never merge from this document's authority**
- **Scope discipline:** docs-only. No live service, DB, or repo was mutated; no pipeline was
  triggered. All live probes were GET/`ls-remote`/`sqlite3 -readonly`.
- **Worktree:** `/home/mkinney/worktrees/gitforge-repo-registration-handoff-20261011` on
  `/dev/nvme0n1p2` (ext4, NVMe SSD, ROTA=0) — **NVMe confirmed**. The live instance's data
  lives on the NAS bundle (`/nas/Temp/repos/GitForge`); only this worktree is NVMe-local.

## 1. Verdict (read this first)

**There is no registration defect. `bigdata-ci/aegis` is fully registered and CI-active.**
All four observed symptoms are explained by two independent product defects and one
deliberate design choice — none of them lose data or require live repair:

| # | Defect | Class |
|---|--------|-------|
| D1 | `gitforge repo --info owner/name` targets a route that does not exist (`GET /api/repos/{owner}/{name}`); axum's router fallback returns 404 for **every** owner/name argument, including the caller's own repos | CLI/API routing bug |
| D2 | Repo and pipeline listings are filtered to `owner_id == caller` **or role admin/maintainer**; `visibility` is never consulted anywhere. The reporting identity (`mkinney`, role `developer`) is neither, so aegis is invisible in every listing | Authz/visibility model gap |
| D3 | `visibility='private'` + "missing vs forbidden collapse into 404" (anti-enumeration, `repo.rs:112-113`) means a non-owner **cannot distinguish** "unregistered" from "not yours". This is why the discrepancy looked like a registration bug | Deliberate design (documented) |
| D4 | Git HTTP serving has **no inbound authentication** (anonymous read *and* write; ref-policy is the only gate). This is why unauthenticated `ls-remote` succeeds on a private repo | Security observation — separate workstream |

## 2. Observed evidence (all reproduced read-only, 2026-10-11 ~18:1xZ)

| Observation | Result |
|---|---|
| `git ls-remote http://localhost:42782/bigdata-ci/aegis.git` (no credentials) | 200; `28930cfe29e05fe8944999cbbf54cc0780ba452e` at `refs/heads/codex/aegis-diff-eof-failclosed-20261011` + ~20 more refs |
| Negative control: `ls-remote …/bigdata-ci/definitely-not-a-real-repo-xyz.git` | 404 "repository not found" — the server discriminates per-repo |
| `GET /health` on 42780 / 42781 / 42782 | 200 / 200 / 200 |
| `GET /api/repos` as CLI identity | 21 repos, **0** matching "aegis" |
| `GET /api/repos/mkinney/gitforge` (owner/name in id-keyed route — reproduces CLI `--info`) | **404** — for a repo that exists and is owned by the caller |
| `GET /api/repos/mkinney/gitforge/policy` | 200 — owner/name resolution + owner authz work |
| `GET /api/repos/bigdata-ci/aegis/policy` | 404 `{"error":"not_found","message":"repository not found"}` (ambiguous by design, see D3) |
| `GET /api/pipelines` as CLI identity | 32 pipelines, none for aegis (same authz filter) |
| CLI JWT claims (`~/.config/gitforge/config.toml`, decoded locally; token **not** printed) | `username=mkinney`, `role=developer`, `user_id=e2443771-cf6d-45a4-8d09-fb5be75823c8`, valid until 2026-10-12T16:49Z |
| `sqlite3 -readonly /nas/Temp/repos/GitForge/data/gitforge.db` | `repositories`: `d815b2cc-0f2c-416d-ba15-a0038d36a9db`, name `aegis`, **private**, git_path `/nas/Temp/repos/GitForge/repos/d815b2cc-…`, owner `bigdata-ci`, created 2026-09-22T19:39:05Z |
| same, `users` | `bigdata-ci` = user id `484d6179-220f-49f1-a09a-42ee62528768`, **role admin**, created 2026-09-17 |
| same, `pipelines` / `pipeline_runs` | **260 runs**; two pipeline families (`<repo-uuid>-pipeline`, `aegis-release`); active rows: `74169605…` (since 2026-09-22) and `230a5c5a…` `aegis-release` created **2026-10-11T17:48:04Z** — pushes are landing *today* |
| Shared env | All three template units load `EnvironmentFile=-/nas/Temp/repos/GitForge/.env`; `DATABASE_URL=sqlite:///nas/Temp/repos/GitForge/data/gitforge.db?mode=rwc` — one DB for api+ci+git-server |
| Running release | `releases/gitforge-current` → `gitforge-4d72efc2-20261010`; `4d72efc2` **is an ancestor** of worktree HEAD `8431f117` |

## 3. Source trace (file:line; first-hand reads unless noted "trace")

### 3.1 Identity / owner / path mapping
- Owners are **users only** — there is no organizations table anywhere (`migrations/001_initial_schema.sql:8-14`; trace: grep over crates/services/migrations for `organization|org_id` → 0 hits). `bigdata-ci` is an ordinary `users.username`.
- URL `/{owner}/{repo}.git` maps through `JOIN users u ON r.owner_id = u.id WHERE u.username = ? AND r.name = ?` — `crates/gitforge-db/src/queries.rs:444-466`.
- Storage is keyed by **repo UUID, not owner/name**: `<GIT_ROOT>/<RepoId>` — `crates/gitforge-core/src/storage.rs:85-87` (trace). The live row confirms: `/nas/Temp/repos/GitForge/repos/d815b2cc-…`. `repositories.git_path` is stored (`001_initial_schema.sql:28-36`) but **never used for resolution**.
- No `UNIQUE(owner_id, name)` on `repositories` (`001_initial_schema.sql:28-39`); only app-level `validate_repo_name` guards names (rejects `/` since the v0.6.13 fix, `crates/gitforge-api/src/routes/repo.rs:23-72`). A pre-0.6.13 slash-name row could NOT match `get_by_owner_and_name('bigdata-ci','aegis')` and would 404 on git too — **ruled out** by observation.
- Usernames are provisioned out-of-band (no `/users` registration route; trace) and unvalidated; case-sensitive SQL `=` match.

### 3.2 Git serving is DB-first (the decisive mechanism)
- `services/git-server/src/main.rs:343-378` (`git_info_refs`): trims `.git` (`:348`), then `lookup_repo_id` → `RepoQueries::get_by_owner_and_name` (`:237-260`, trace); **404 if no row, before disk is touched**; 404 again if `storage.exists(repo_id)` fails (`:373`); 503 if no DB pool.
- Pinned tests: `services/git-server/tests/git_http_edges.rs:357-374` (unknown name → 404) and `:383-410` (row without storage → 404).
- Therefore **an unauthenticated ls-remote success is positive evidence that the `(users.username, repositories.name)` row exists** in the shared DB. The negative control confirms the live binary behaves this way; DB-first resolution dates to at least `e91a53c2` (2026-09-08), far predating the running `4d72efc2` release.
- The only production path that creates a bare dir + DB row together is `create_repo` (`crates/gitforge-api/src/routes/repo.rs:212-306`, incl. compensating storage delete at `:286`). No mirror/restore/bundle path creates bare dirs (trace). An "orphaned dir serving refs" is impossible on this code.

### 3.3 API surface, auth, visibility
- Entire repo route table — `crates/gitforge-api/src/routes/repo.rs:94-106`: `GET/POST /repos`, `GET/DELETE /repos/{id}` (UUID), `GET/PATCH /repos/{owner}/{repo}/policy`, `GET /repos/{owner}/{repo}/commits/{sha}/status`. **There is no `GET /repos/{owner}/{name}` info route.**
- `can_access_repo` (`repo.rs:108-110`): `claims.user_id == owner_id || role in {"admin","maintainer"}`. `visibility` never consulted — a public repo is equally hidden from non-owners.
- `resolve_policy_repo` (`repo.rs:114-143`): deliberately collapses missing+forbidden → 404 (`:112-113` "so existence is not leaked"). Auth middleware always yields 401, never 404 (`crates/gitforge-api/src/middleware.rs:46-91`, trace).
- Listings: SQL is unfiltered (`queries.rs:429-441`), filtered per-row in handlers by `can_access_repo` (`repo.rs:184`) and `authorize_repo` (`crates/gitforge-api/src/routes/ci.rs:112-124`, silent drops at `ci.rs:163`/`:871`, trace).

### 3.4 CLI call path (D1's exact mechanism)
- `gitforge repo --info <arg>` → `client.get_repo(arg)` interpolates the **raw** argument into `{base}/api/repos/{id}` — `crates/gitforge-cli/src/client.rs:343-355`. With `bigdata-ci/aegis` this becomes a two-segment path matching **no route** → axum default fallback → 404 with empty body. The handler-level anti-enumeration logic is never reached.
- Proof this is identity-independent (live): the same request for the caller's **own** repo, `GET /api/repos/mkinney/gitforge` → 404, while `mkinney/gitforge` appears in the caller's listing and its policy route returns 200.
- `--policy` / `--commit-status` are name-addressable (`client.rs:357-364`); `--info` is the only broken one. Statuses are printed verbatim (`client.rs:16-30`, trace) — "404 Not Found" with no reason.

### 3.5 Pipeline registration (why "no pipelines" is meaningless for non-owners)
- Pipelines auto-create on first processed HTTP push, not at repo creation, `.gitforge.yml` optional (default pipeline synthesized; trace: `services/ci/src/main.rs:2209-2324`, `:2991`; `queries.rs:702`, `:746-780`).
- Runs FK to `repositories(id)` with `PRAGMA foreign_keys=ON` (`crates/gitforge-db/src/connection.rs:157`, `:281`, `:315-316`, asserted `:913`) — **a run cannot exist for an unregistered repo**. 260 runs exist for aegis.
- No reconciliation/backfill registers repos into CI (trace); unregistered-repo triggers die in `pipeline_trigger_requests` after 5 attempts (`services/ci/src/main.rs:83`, `:2209-2226`, `:2139-2152`).
- **SSH pushes never trigger CI** (`services/git-server/src/ssh_server.rs:336-431` has no `enqueue_ci_event`/HTTP call — trace). Silent transport asymmetry; separate defect (D-series below).

### 3.6 Live instance topology
- systemd template units `gitforge@api/ci/git-server.service` (`User=gitforge`, `ExecStart=…/releases/gitforge-current/bin/%i`, shared `.env`); `gitforge@runner` masked (expected, see release-channel memory). `/proc/<pid>/environ` unreadable to mkinney — closed instead via the shared `.env` (single `DATABASE_URL`).

## 4. Causal chain of the incident

1. Co-tenant agent(s) push to `bigdata-ci/aegis` (registered 2026-09-22 by the `bigdata-ci` admin account; pipelines created seconds after first push).
2. An operator/agent authenticated as `mkinney` (role `developer`) ran `repo --info bigdata-ci/aegis` → D1 made that a guaranteed 404 regardless of registration state.
3. Repo/pipeline listings omitted aegis because of D2 (developer ≠ owner `bigdata-ci`, not admin/maintainer).
4. Unauthenticated `ls-remote` succeeded because serving is DB-first + anonymous (D4) — this looked contradictory but is actually the strongest evidence *for* registration.
5. D3 made the 404 uninterpretable from a non-owner vantage: forbidden and missing are indistinguishable by design.

**Net:** two product defects (D1, D2) conspired to make a healthy repo look unregistered. No data was ever at risk; no repair of registration state is required.

## 5. Preservation / no-data-loss constraints

- **The DB row is the only map** from `bigdata-ci/aegis` → storage `d815b2cc-…`. Never recreate the row (new id = orphaned storage + broken run FKs); never re-`init` the bare dir; never "fix" by deleting either side.
- `pipelines`/`pipeline_runs` for aegis (260 runs, families since 2026-09-22) are history/audit; deletion or re-registration would destroy them. Runs imply the repo row via FK.
- **Aegis is live**: `aegis-release` pipeline activated 2026-10-11T17:48:04Z, ~30 min before this probe. Co-tenant agents are pushing now — coordinate (agent-locks discipline) before anything beyond read-only.
- SQLite is single-writer WAL on NAS: never migrate or write while a peer runs (ledger F40 stale-prepared-statement panic class). All diagnostic reads `-readonly`.
- Never force-push; no `git reset --hard`/`clean -fd` on any live or co-tenant checkout.
- Non-owner identities have no legitimate write path to aegis — do not attempt to "adopt" it via another account.

## 6. Idempotent repair design (phased; P0 already executed read-only)

**P0 — Diagnosis (DONE, read-only).** Findings in §2. Re-run anytime:
```
curl -s -H "Authorization: Bearer $TOK" http://localhost:42780/api/repos | jq '[.[]|select(.name=="aegis")]'
curl -s -w '%{http_code}\n' -H "Authorization: Bearer $TOK" http://localhost:42780/api/repos/mkinney/gitforge   # 404 = D1 control
git ls-remote http://localhost:42782/bigdata-ci/definitely-not-a-real-repo-xyz.git                              # 404 negative control
sqlite3 -readonly /nas/Temp/repos/GitForge/data/gitforge.db "SELECT r.id,r.name,r.visibility,u.username FROM repositories r JOIN users u ON r.owner_id=u.id WHERE r.name='aegis';"
```

**P1 — Fix D1 (code PR; additive, idempotent, no DB change).**
1. API: add `GET /repos/{owner}/{name}` handler reusing `resolve_policy_repo` (`repo.rs:114-143`) so owners/admins can fetch info by name and non-owners keep the collapsed 404. Route goes in `repo_routes` (`repo.rs:94-106`) **before** `/repos/{id}` is affected — axum matches literal-extra-segment paths distinctly, but add a router test pinning `/repos/{uuid}` and `/repos/{owner}/{name}` both resolve.
2. CLI: `--info` dispatches on argument shape — UUID → legacy `GET /repos/{id}`; `owner/name` → new endpoint (mirror `bare_repo_name`/policy-arg handling, `main.rs:280-282`).
3. Document the collapse in `docs/API.md` next to the policy route.

**P2 — Decide D2 (product decision, then small PR or docs).** Options: (a) keep owner/admin-only listings and document that `developer` roles see only their own repos (status quo, zero code); (b) let `visibility='public'` make repos readable/listable by any authenticated user (touches `can_access_repo`, `authorize_repo`, listing filters, and — for consistency — git serving auth, which currently has none, see D4). Do **not** half-adopt (b) without deciding D4.

**P3 — DB-split convergence: N/A.** Ruled out by the shared `.env`/`DATABASE_URL`. If a future split is ever suspected, the operator check is `sudo systemctl cat gitforge@{api,ci,git-server}` + diff of `DATABASE_URL`.

**P4 — Orphan-adoption runbook (contingency only; P0 shows it is NOT needed).** Kept for robustness if a future P0 ever finds a served name with no row: as operator, create the `users` row if absent → `INSERT repositories` (name/owner/visibility, fresh `RepoId`) → `storage.create(repo_id)` (bare init at `<GIT_ROOT>/<uuid>`) → `git push --mirror file://<old-dir>` (idempotent by SHA) → verify `ls-remote` advertises a superset incl. the recorded SHA → leave the original dir untouched (revert = DELETE row + remove the new dir). Gate behind operator approval + agent-locks + a drained CI.

**P5 — D4 security workstream (separate, blocker-grade recommendation, out of scope here).** Git HTTP on 0.0.0.0:42782 accepts unauthenticated read **and write**; only ref-policy gates updates. Private visibility is unenforced at the git layer. Recommend deploy-token/PAT auth on receive-pack at minimum. Do not bundle into P1/P2.

## 7. Test plan (applies to the P1/P2 code PRs; this docs PR needs none)

- **API integration** (`crates/gitforge-api/tests/`): new info-by-name route → 200 for owner; 404 (assert collapse, not 403) for other-`developer`; 404 for missing owner or name; 401 unauthenticated; UUID route unchanged.
- **CLI unit** (`crates/gitforge-cli`): `--info` URL construction — UUID → `/repos/{id}`; `owner/name` → `/repos/{owner}/{name}`; regression test that an unowned name prints the raw 404 (current behavior) until P1 ships.
- **Pins already in tree**: `services/git-server/tests/git_http_edges.rs:357-374` (unknown name 404), `:383-410` (storage-less row 404) — keep green; they encode the DB-first contract this diagnosis leaned on.
- **D3 follow-up test** (separate defect): SSH push enqueues a CI trigger with the same payload as HTTP (currently expected to fail — write the test when fixing).
- **Gates** (mirror `.gitforge.yml`): `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`; `cargo test --workspace -- --test-threads=2`. Coverage gate 82/84 stands.

## 8. Rollback

- **This docs PR:** single-commit docs addition; `git revert <sha>`; no runtime effect. Pushing this branch triggers nothing (`.github/workflows/gitforge-ci.yml` pushes fire on `main`/`stable` only; `pull_request` filter is code paths only).
- **P1/P2:** additive route + CLI dispatch — revert the commit; no data touched; no migration.
- **P4 (contingency):** revert = `DELETE` the inserted row + remove the created UUID dir after SHA-verified mirror is confirmed elsewhere; the pre-existing dir is never modified.
- **P5:** purely additive middleware; rollback = disable at config; no schema.

## 9. Ownership

| Item | Owner |
|---|---|
| This handoff + draft PR | This agent (docs-only; never merges) |
| Filing D1/D2/D3/D4 into the F-series ledger | Operator (ledger is append-with-verdict; not edited by agents without evidence per `CLAUDE.md`) |
| P1/P2 implementation | Repo maintainer (mkinney) / gitforge-api+cli owners |
| P4 execution (if ever) | Operator only (`sudo`, `gitforge` user, drained CI, agent-locks) |
| P5 security workstream | Operator + security owner; schedule before any non-LAN exposure |

## 10. Phased acceptance criteria

- **A0 (this PR):** doc committed on `docs/gitforge-repo-registration-handoff-20261011`, pushed to `origin`, **draft** PR opened against `main`; no pipeline triggered anywhere; all §2 probes reproducible by a later agent.
- **A1 (P1):** `GET /api/repos/bigdata-ci/aegis` as `bigdata-ci` → 200 with repo JSON; same call as `mkinney` → 404; `repo --info bigdata-ci/aegis` as owner succeeds; as developer prints 404 (documented); full local gate chain green.
- **A2 (P2):** whichever option is chosen, its behavior is pinned by tests and stated in `docs/API.md`; `visibility` is either honored everywhere or declared inert in one place — no in-between.
- **A4 (contingency only):** after adoption, `ls-remote` advertises ≥ the pre-adoption SHA set incl. `28930cfe29e05fe8944999cbbf54cc0780ba452e`; 260 runs still resolve; original dir mtime/contents untouched.
- **A5 (P5):** unauthenticated `git ls-remote` and push against any private repo fail with 401; authenticated flows unaffected; ref-policy tests green.

## 11. Evidence gaps

1. **Which pipeline family is "current" for new aegis pushes** — two active rows coexist (`<uuid>-pipeline` since 2026-09-22, `aegis-release` since 2026-10-11T17:48Z). Allowed by `idx_pipelines_active_repo_name` (unique per (repo, name), `connection.rs:291-295`), but intent is unknown; possible config duplication by co-tenants. Not a defect; worth an owner look.
2. **Transport of recent aegis pushes (HTTP vs SSH)** — if SSH, D3 means no CI triggers fired for them; `pipeline_trigger_requests` rows would settle it (not queried; probe kept bounded).
3. **`gitforge repo --info` not executed as the CLI binary** (avoiding config mutations per the auth-restore memory); reproduced at the exact HTTP layer the CLI emits (`client.rs:343-355`) plus the owned-repo control.
4. **Owner-side confirmation** (policy 200 as `bigdata-ci`) not performed — no credentials for that account; the DB row + serving behavior substitute.
5. **codebase-memory MCP tools were unavailable** in this agent; per instruction, fell back to bounded first-hand reads + three read-only Explore traces, all cited by file:line.
6. **Scope of D1 confusion elsewhere** — every repo in the instance is equally "uninfo-able" by name; no sweep attempted (out of scope).
