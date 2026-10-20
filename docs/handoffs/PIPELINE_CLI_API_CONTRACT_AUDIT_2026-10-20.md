# Pipeline CLI/API Contract Audit — Receipt 2026-10-20

## Outcome

Three reported symptoms were reproduced against the source and classified.
**Two are proven source defects and are fixed on this branch; one is the
documented contract and is left as it is.**

| Reported symptom | Classification | Disposition |
|---|---|---|
| `pipeline --list` showed a repository UUID where the pipeline id belonged | **Proven defect** — the CLI printed `repo_id` in the pipeline-id slot | Fixed |
| `pipeline --show <that uuid>` answered `404` | **Proven defect, same root cause** — the id routes resolve against the `pipelines` table, so a repository uuid is "not found" | Fixed |
| `pipeline --list`/`--show` labelled every pipeline `disabled` | **Proven defect** — the CLI read a field the API never sent | Fixed |
| `pipeline --show <name>` answered `invalid_id` | **By design** — the id routes take a pipeline id (uuid); a name is not an identifier | Documented, no code change |

The fix is source-level only. No production pipeline, secret, service, or
deployment was touched.

## Scope and safety

- **Worktree**: `/home/mkinney/agent-work/gitforge-pipeline-contract-20261020`
  (NVMe-backed), branch `codex/pipeline-contract-audit-20261020`, base commit
  `114c69f` (`feat(ci): expose authenticated trigger receipts (#317)`).
- **Production**: not mutated. No pipeline was enabled, retired, deleted, or
  triggered; no service was restarted; no secret was read or written; no GPU
  was used.
- **Tests**: focused suites only (`gitforge-db`, `gitforge-api`, `gitforge-cli`
  targets listed under *Verification*). No workspace-wide `cargo test`, no
  coverage run, no e2e.
- **Concurrency note**: an earlier invocation of this audit was still running
  in the same worktree while this one started; its writes were inspected, not
  discarded, and its changes were folded into the final diff. File mtimes were
  confirmed stable before the commit.

## Evidence

### Symptom 1 — the listing printed an identifier the routes reject

Pre-fix, `gitforge pipeline --list` printed the repository id:

```rust
// crates/gitforge-cli/src/main.rs:776-783 (at 114c69f)
let status = if pipeline.enabled { "active" } else { "disabled" };
println!(
    "  {:20} - {} [{}]",
    pipeline.name, pipeline.repo_id, status
);
```

The API had already been sending the pipeline id in the same payload
(`crates/gitforge-api/src/routes/ci.rs`, list route: `id`, `repo_id`, `name`,
`trigger_type`, `created_at`) — the CLI simply printed the wrong field.

Feeding that repository uuid back to the id route reproduces the reported
`404` from source alone: `GET /api/pipelines/{id}` parses the path as a uuid
(a repository id *is* a uuid, so parsing succeeds), then looks the value up in
the `pipelines` table (`crates/gitforge-api/src/routes/ci.rs:238`), finds no
row, and answers `404 {"error":"not_found","message":"Pipeline not found"}`.

### Symptom 2 — the name lookup answered `invalid_id`

`GET /api/pipelines/{id}` requires a uuid
(`crates/gitforge-api/src/routes/ci.rs:235`, `Uuid::parse_str`), and a
non-uuid path answers `400 {"error":"invalid_id","message":"Invalid pipeline ID
format"}`. This is the contract working as designed, not a defect:

- a pipeline **name** is unique only per repository, and
- retired versions share the name with their successor (the single-active-
  version invariant, `crates/gitforge-db/src/queries.rs:718-737`),

so a name does not identify a row. The id is the only identifier the routes
accept. The audit therefore did **not** add a name lookup route; it made the
listing print the id that the routes accept, and the CLI help now says so.

### Symptom 3 — every pipeline was labelled `disabled`

The CLI decoded a field the API never sent:

```rust
// crates/gitforge-cli/src/client.rs:120-123 (at 114c69f)
/// The API list endpoint does not return `enabled`; default it so the
/// response still decodes (the run/list flows only need id/name/repo_id).
#[serde(default)]
pub enabled: bool,
```

With `#[serde(default)]` and no `enabled` key in the payload, every pipeline
decoded as `enabled = false`, and the CLI printed `disabled` for all of them.
The database had the truth all along — `pipelines.active`
(`crates/gitforge-db/src/connection.rs:273-283`) — but no route exposed it.

### The state the API already stored

`pipelines.active` is `INTEGER NOT NULL DEFAULT 1`; the partial unique index
`idx_pipelines_active_repo_name` admits one active row per
`(repo_id, name)`. Registering or pushing a new version retires the
predecessor (`active = 0`), which stays addressable by id as history. The
catalog listing already carried only active rows
(`crates/gitforge-db/src/queries.rs:869`), which is why the reported listing
showed a `disabled` pipeline that the API itself never listed: the label was
fabricated by the client.

## Changes

### `crates/gitforge-db/src/queries.rs`

- `PipelineQueries::get_with_state` (line 847) — one read that returns the
  definition and the `active` flag *from the same row*, so the two cannot
  come from two different versions of it.
- `PipelineQueries::active_flags` (line 911) — one read that serves a whole
  listing, scoped to the requested ids (`WHERE id IN (...)`) so a listing does
  not drag the version history of every repository on the instance through the
  connection.
- `read_active_flag` (line 948) — reads the flag column; any non-zero value
  counts as active, so a future flag value cannot silently read as retired.

### `crates/gitforge-api/src/routes/ci.rs`

- List route: keeps only the pipelines the caller may see, reads the flags for
  exactly those rows, and reports `active` per row (line 205). A row the flag
  read did not return is no longer in the table, so it cannot be an active
  definition.
- Get route: reports `active` from the row it read (line 251). A retired
  version stays addressable by id and now says so.
- Create route: the registered definition *is* the active version of its
  `(repository, name)` pair, and the response says so (line 456).

### `crates/gitforge-cli/src/`

- `client.rs:126` — `PipelineResponse.active: Option<bool>`. The API reports
  the flag; a server that predates the field leaves it absent, and the CLI
  says `unknown` rather than asserting a state nobody sent.
- `main.rs:292` — `pipeline_state_label`: `active` / `disabled` / `unknown`.
- `main.rs:306` — `format_pipeline_line`: the row carries the **pipeline id**
  (the handle `--show`, `--run`, and `--delete` accept) next to the repository
  id, plus the reported state.
- `--show`, `--run`, and `--delete` help text now says "by pipeline ID (from
  --list)".

### Documentation

- `docs/API.md` — the pipeline payload contract: the fields, that `id` is the
  handle the id routes accept, that a repository id answers `404` and a name
  answers `400`, and what `active` means.
- `crates/gitforge-api/src/openapi.rs` — the same contract on the `/pipelines`
  and `/pipelines/{id}` operations, including the `400` response the id route
  already returned but never documented.

## Regression tests

| Test | File:line | Pins |
|---|---|---|
| `test_pipeline_active_flag_reports_retired_versions` | `crates/gitforge-db/tests/integration.rs:232` | the flag of a retired vs. active row, that `active_flags` is scoped to the requested ids, and that an empty request is answered without a query |
| `pipeline_responses_report_the_active_state` | `crates/gitforge-api/tests/ci_routes.rs:208` | list/get report `active`, a retired row reports `active: false` and leaves the listing, and the id the listing prints is the id the id route accepts |
| `pipeline_create_registers_active_definition_for_owner` | `crates/gitforge-api/tests/ci_routes.rs:919` | the create response carries `active: true` |
| `pipeline_response_carries_the_state_the_api_sent` | `crates/gitforge-cli/src/client.rs:874` | the client decodes `active: true` / `false` as sent |
| `pipeline_response_does_not_invent_a_state_the_api_never_sent` | `crates/gitforge-cli/src/client.rs:889` | an absent state decodes as "not reported", not as `disabled` |
| `pipeline_state_label_reports_only_what_the_api_sent` | `crates/gitforge-cli/src/main.rs:1273` | `active` / `disabled` / `unknown` |
| `pipeline_list_line_carries_the_pipeline_id_and_the_reported_state` | `crates/gitforge-cli/src/main.rs:1282` | the listing row carries the pipeline id and the reported state |
| `pipeline_list_line_never_labels_an_unreported_state_disabled` | `crates/gitforge-cli/src/main.rs:1291` | an unreported state is never printed as `disabled` |

## Verification

All commands were run in the worktree; the results are verbatim.

```
cargo fmt --all -- --check                                  # clean
cargo clippy -p gitforge-db -p gitforge-api -p gitforge-cli \
    --all-targets -- -D warnings                            # clean
RUSTDOCFLAGS="-D warnings" cargo doc -p gitforge-db \
    -p gitforge-api -p gitforge-cli --no-deps               # clean

cargo test -p gitforge-db --test integration                # 29 passed; 0 failed
cargo test -p gitforge-api --test ci_routes                 # 28 passed; 0 failed
cargo test -p gitforge-api --lib                            # 172 passed; 0 failed
cargo test -p gitforge-api --test integration pipeline      # 12 passed; 0 failed
cargo test -p gitforge-cli --bins                           # 142 passed; 0 failed
cargo test -p gitforge-cli --test cli                       # 49 passed; 0 failed
```

## Not verified, and residual risk

- **No live service run.** The contract is pinned at the route and client
  level; the CLI was not exercised against a running GitForge instance, and
  no request was made to a production API. The reported `404` / `invalid_id`
  answers were derived from the source paths cited above, not replayed
  against production.
- **Coverage gate not measured.** The repository's coverage threshold
  (fail < 82%, warn < 84%, `.gitforge.yml`) is a sandbox measurement; the
  brief scoped this run to focused tests, so the number is not claimed here.
  The authoritative pipeline run remains the gate.
- **Pre-existing N+1 left alone.** The list route issues one authorization
  query per candidate row. That is unchanged by this branch and was out of
  scope; the new flag read is one scoped query, not one per row.
- **Additive API change.** `active` is a new response field. An older CLI
  build against the new API still decodes the absent `enabled` field as
  `false` and will keep printing `disabled` until it is upgraded; the CLI and
  API ship from this repository together.
- **Name lookup still unsupported.** `pipeline --show <name>` answers
  `invalid_id` by design. If name-based lookup is wanted later, it needs a
  repository scope to be unambiguous, and it is a separate contract change.

## Files changed

```
crates/gitforge-api/src/openapi.rs            contract on the documented routes
crates/gitforge-api/src/routes/ci.rs          list/get/create report the state
crates/gitforge-api/tests/ci_routes.rs        route-level regression tests
crates/gitforge-cli/src/client.rs             decode the reported state only
crates/gitforge-cli/src/main.rs               print the pipeline id; honest labels
crates/gitforge-db/src/queries.rs             read the flag from the row
crates/gitforge-db/tests/integration.rs       flag and scoping regression tests
docs/API.md                                   the pipeline payload contract
docs/handoffs/PIPELINE_CLI_API_CONTRACT_AUDIT_2026-10-20.md   this receipt
```
