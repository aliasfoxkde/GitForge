# Tasks: Pipeline CLI/API Contract Audit

Branch: `codex/pipeline-contract-audit-20261020`
Scope: `gitforge pipeline --list/--show/--run` contract against the
`GET /api/pipelines` and `GET /api/pipelines/{id}` routes. No production
pipeline, secret, or service mutation.

Evidence receipt: `docs/handoffs/PIPELINE_CLI_API_CONTRACT_AUDIT_2026-10-20.md`

## Task Checklist

### Phase 1: Audit (evidence only)
- [x] Read AGENTS.md, CLAUDE.md, and the CI pipeline contract sources
- [x] Trace `gitforge pipeline --list` output fields to the API payload
- [x] Trace `gitforge pipeline --show/--run/--delete` argument to the API route
- [x] Trace pipeline active/retired state through db queries and API responses
- [x] Classify each reported symptom (404 on repo UUID, invalid_id on name, disabled label)

**Verdicts**

| Symptom | Classification | Proof |
|---|---|---|
| `--list` shows a repository UUID where the pipeline id belongs | proven defect | `crates/gitforge-cli/src/main.rs:776-783` at `114c69f` printed `pipeline.repo_id` while the API payload already carried `id` |
| `--show <that uuid>` → 404 | proven defect, same root cause | `crates/gitforge-api/src/routes/ci.rs:238` resolves the path against the `pipelines` table; a repository uuid parses as a uuid and matches no row |
| `--show <name>` → invalid_id | by design, not a defect | the id routes require a uuid (`crates/gitforge-api/src/routes/ci.rs:235`); a name is unique only per repository and is shared by retired versions |
| every pipeline labelled `disabled` | proven defect | `crates/gitforge-cli/src/client.rs:120-123` at `114c69f` decoded an `enabled` field the API never sent, defaulting it to `false`; the truth is `pipelines.active` (`crates/gitforge-db/src/connection.rs:273-283`) |

### Phase 2: Fix (only if a defect is proven)
- [x] Fix the fabricated pipeline state shown by `pipeline --list`/`--show`
- [x] Fix the identifier printed by `pipeline --list` so it is the one `--show`/`--run` accept
- [x] Expose the pipeline active state in the API payloads that report it
- [x] Pin the contract with regression tests (db, api, cli)

**Changes**

- db: `PipelineQueries::get_with_state` (definition + flag from one row),
  `PipelineQueries::active_flags` (one read per listing, scoped to the
  requested ids), `read_active_flag`.
- api: list/get report `active`; create reports `active: true`.
- cli: `PipelineResponse.active: Option<bool>`; labels `active`/`disabled`/
  `unknown`; the listing row prints the pipeline id; `--show`/`--run`/`--delete`
  help says "by pipeline ID (from --list)".
- docs: `docs/API.md` payload contract; `openapi.rs` route descriptions.

### Phase 3: Verification and handoff
- [x] Run focused tests (gitforge-db, gitforge-api, gitforge-cli)
- [x] Run fmt + clippy on the touched crates
- [x] Write the evidence handoff in docs/handoffs/
- [x] Commit, push the branch, and open a draft PR

**Verification (verbatim)**

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy -p gitforge-db -p gitforge-api -p gitforge-cli --all-targets -- -D warnings` | clean |
| `RUSTDOCFLAGS="-D warnings" cargo doc -p gitforge-db -p gitforge-api -p gitforge-cli --no-deps` | clean |
| `cargo test -p gitforge-db --test integration` | 29 passed; 0 failed |
| `cargo test -p gitforge-api --test ci_routes` | 28 passed; 0 failed |
| `cargo test -p gitforge-api --lib` | 172 passed; 0 failed |
| `cargo test -p gitforge-api --test integration pipeline` | 12 passed; 0 failed |
| `cargo test -p gitforge-cli --bins` | 142 passed; 0 failed |
| `cargo test -p gitforge-cli --test cli` | 49 passed; 0 failed |

## Not done (deliberately)

- No name-based pipeline lookup (a name is not an identifier; adding one
  needs a repository scope and is a separate contract change).
- No workspace-wide `cargo test` and no coverage measurement — the brief
  scoped this run to focused tests; the GitForge pipeline remains the
  coverage gate.
- No production instance was exercised: the live check ran against a
  throwaway local API instance on a scratch SQLite database with synthetic
  rows (see the receipt), and no production service, secret, or pipeline
  was contacted or mutated.