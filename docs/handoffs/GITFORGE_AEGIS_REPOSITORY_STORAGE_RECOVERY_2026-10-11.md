# GitForge Aegis Repository Storage Recovery — Execution Handoff (2026-10-11)

**Status:** draft handoff awaiting operator execution. Docs-only; nothing in
this document has been executed against the live deployment.
**Source pin:** every code anchor below was read at
`8431f1170608bc8fc86862777e2619bdfa5d8387` (GitHub `main`, 2026-10-11).
Re-anchor line numbers against the executing checkout before quoting them in
receipts — the old local branch's numbering has already drifted in one place
(`get_git_root`, see §6).

**Evidence labels used throughout:**

- **[OBSERVED]** — a dated operator/session observation from 2026-10-11,
  quoted here. The author of this handoff could **not** independently
  reproduce any [OBSERVED] item from this worktree; they require
  remeasurement (§5, §14).
- **[VERIFIED-SOURCE]** — read directly from the pinned source tree during
  this session; reproducible from the pin.
- **[HYPOTHESIS]** — explicitly unproven; each carries a discriminating test.

---

## 1. Scope and impact

Two (at minimum) GitForge repository rows reference bare-repo storage paths
under the retired audit root `/home/mkinney/work/gitforge-audit-20260822/`
that are **absent from the filesystem**. Their registered pipelines can no
longer run: the gateway refuses a run trigger with HTTP 500
`storage_unavailable` before any revision is resolved or anything is
enqueued **[VERIFIED-SOURCE]** (§6). Any other repo row pointing under the
same retired roots is presumed equally affected until the census in Phase 1
proves otherwise — the spot-checks so far are not a census.

Impact is currently **availability and evidence loss-risk**, not corruption:

- No data has been mutated by anyone in response to this incident. No
  restore, re-registration, DB edit, trigger, or recreation has occurred
  **[OBSERVED, 2026-10-11]**.
- The Aegis repos are validation-evidence carriers (their names date them to
  the 2026-08-22 audit and 2026-08-31 validation campaigns). Losing their
  unique refs would destroy the immutable evidence those campaigns produced,
  which is why this handoff is preservation-first and recovery-second.

Out of scope for this handoff: any code change; any change to service
configuration, the database, storage paths, GitForge refs, or pipelines;
the optional `storage_unavailable`/path-redaction code follow-ups (§10
keeps them explicitly separate).

## 2. Observed evidence (2026-10-11)

All items in this section are **[OBSERVED]** unless marked otherwise.

### 2.1 Registered repositories with absent storage

| Repository (name) | UUID | Registered `git_path` | State | Linked pipeline |
|---|---|---|---|---|
| `aegis` | `b09d49f8-f1ba-4045-8305-3ab7c0a071ca` | `/home/mkinney/work/gitforge-audit-20260822/runtime-repos/b09d49f8-f1ba-4045-8305-3ab7c0a071ca` | path absent | `aegis-release` (`7eb6a1a1-d948-4d26-8e42-cdddfeffc03d`) |
| `aegis-platform-validation-20260831-r4` | `d1326cfb-e959-4e7d-bfbd-527eb2f97dcd` | absent registered path under the same `runtime-repos` parent | path absent | `aegis-validation` (`04ca0bb1-25ed-4624-ab84-dde6e4de505c`) |

Other registered Aegis repos under the same retired
`/home/mkinney/work/gitforge-audit-20260822/` roots were spot-checked
absent. **This is not yet a complete census** — Phase 1 exists precisely
because spot-checks do not bound the problem.

### 2.2 Service/runtime observations (same day — now stale)

- GitForge API, CI, and Git health endpoints returned HTTP 200.
- `gitforge-runner` systemd unit: active, `NRestarts=0`, started
  2026-10-10 12:44 CDT.
- Journal, trailing 24h: 2,484 successful / 64 failed jobs.
- Reconciliation reported 216 candidates, 0 eligible, 0 removed, deletion
  disabled. Note **[VERIFIED-SOURCE]**: this is the *sandbox container*
  reconciler (`crates/gitforge-sandbox/src/reconciler.rs:1-30`) — census by
  default, removal only when `ReconcilerPolicy::deletion_enabled` is set.
  It reconciles **containers**, not repository storage; its healthy report
  says nothing about repo directories.

**Network subnet changed since these observations.** The address used on
2026-10-11 (reported as `192.168.1.202`) must not be assumed current. Every
execution step in this handoff requires a **fresh address discovery** (§8,
Phase 0) before any endpoint contact; no command in this document hardcodes
the old address.

### 2.3 Why health checks do not clear this incident

Health endpoints, job counters, and unit liveness prove the **processes**
are alive and scheduling work. They cannot prove storage integrity, because
the read path that fails here is per-repository and lazy: `RepoQueries::list`
returns whatever rows exist without touching the filesystem, and the
`git_path` is hydrated straight from the DB row
(`crates/gitforge-db/src/queries.rs:429-441`, `:118-141`) — so `/api/repos`
lists a repo whose directory is gone. The failure only surfaces when
something needs the path: a run trigger checks `repo.git_path` existence and
returns 500 `storage_unavailable`
(`crates/gitforge-api/src/routes/ci.rs:508-518`); a Git smart-HTTP request
for an unbacked repo 404s — both the unknown-repo case and the
row-exists-but-storage-absent ("orphan row") case are pinned as tests in
`test_unknown_and_unbacked_repositories_are_not_served`
(`services/git-server/tests/git_http_edges.rs:353-412`; unknown repo
`:359-382`, storage-less row `:384-409`). A 200 on `/health` and a 200 on
`/api/repos` are therefore **liveness, not storage truth**. Recovery must
never be declared complete from them.

## 3. What is *not* broken (as of the dated observations)

Stated with the same staleness caveat as §2.2 — each item is "was healthy
when observed on 2026-10-11", not "is healthy now":

- Service liveness and job execution broadly (2,484 successful jobs/24h)
  — i.e. CI/runner scheduling works for repos whose storage exists.
- Authentication, the API/CI/Git HTTP planes, and SSH were serving.
- The container reconciler is operating fail-closed (deletion disabled).
- GitHub-side development of this repository (branches, PRs) is unaffected.
- No evidence of DB corruption; no evidence of any write attempt against
  the affected rows.

Absence of a directory is **not** evidence of how it came to be absent.
No attribution (deletion, move, host event) is made anywhere in this
handoff.

## 4. Explicit unknowns

1. Complete census: how many of all registered repos have missing,
   malformed, or non-bare storage; how many orphan directories exist.
2. Whether **any** copy of the two named repos exists anywhere: bundles,
   dated backup dirs, other storage roots, Git worktrees, the GitHub
   mirror, or the Aegis source checkouts — and if copies exist, whether
   they carry **all** refs (platform repos may hold refs a source checkout
   never had; the inverse is also unproven).
3. Row contents for the affected repos (owner, visibility,
   `required_checks`, `deny_non_fast_forward`, timestamps) and the
   pipelines' `config` payloads — not yet captured.
4. The live deployment's actual `DATABASE_URL`, `GITFORGE_DATABASE_URL`,
   `GIT_ROOT`, `GITFORGE_ARTIFACT_ROOT`, and port values (env var *names*
   are [VERIFIED-SOURCE], §6; values were never captured).
5. Whether the retired parent root `/home/mkinney/work/gitforge-audit-20260822/`
   still exists at all, and what else lives under it.
6. Historical CI runs/logs/artifacts for these repos (scheduler DB,
   artifact root) — unexamined; they may hold the only remaining evidence
   of the validation campaigns even if repo objects are lost.
7. Current Fedora host address (subnet changed post-observation).
8. Root cause of the absence.

## 5. Hypotheses (unproven — each with a discriminating test)

- **H1 — removed during a post-campaign cleanup.** The 2026-09-20 audit
  receipt (`docs/handoffs/REPO_STATE_AUDIT_2026-09-20.md`) documents
  deliberate branch/worktree cleanup campaigns; a later cleanup could have
  swept runtime dirs. Discriminator: Phase 1 candidate search for dated
  cleanup scripts/logs and deletion receipts; shell history and journal.
- **H2 — storage moved to a new root during a deployment/host change.**
  Discriminator: filesystem-wide search for the two UUID directory names;
  compare against any newer `GIT_ROOT` values in service env.
- **H3 — host reinstallation/reprovision.** The subnet change is
  consistent with host/network work. Discriminator: filesystem birth
  dates, backup images, operator recollection of reinstalls.
- **H4 — partial loss (only these dirs).** Discriminator: the Phase 1
  census itself — if only Aegis-era rows are missing, loss was scoped;
  if the census is broadly clean, H1–H3 gain weight.

Do not act on any hypothesis. Phase 1 discriminates; Phase 2 decides.

## 6. Code contracts relevant to this incident

All **[VERIFIED-SOURCE]** at `8431f1170608bc8fc86862777e2619bdfa5d8387`:

| Contract | Anchor |
|---|---|
| Run trigger rejects missing storage with 500 `storage_unavailable` **before** revision resolution and CI enqueue (probing it enqueues nothing) | `crates/gitforge-api/src/routes/ci.rs:423-578` (check at `:508-518`) |
| `RepoQueries::list` returns all rows, hydrating each — no path validation | `crates/gitforge-db/src/queries.rs:429-441` |
| `hydrate_repository` loads persisted `git_path` from the DB row | `crates/gitforge-db/src/queries.rs:118-141` |
| `get_git_root`: `GIT_ROOT` env, dev fallback `target/gitforge-repos` | `services/git-server/src/main.rs:930-934` |
| `FileStorageBackend::repo_path` maps repo UUID → `<storage root>/<uuid>` | `crates/gitforge-core/src/storage.rs:85-87` |
| Repo policy `deny_non_fast_forward` is enforced by writing `receive.denyNonFastForwards` into the repo's local git config — repo config carries policy and is part of parity | `crates/gitforge-core/src/storage.rs:70-80` |
| Env var names: `DATABASE_URL` (api `services/api/src/main.rs:182`, git-server `:82`), `GITFORGE_DATABASE_URL` (ci `services/ci/src/main.rs:188`), `GIT_ROOT` (git-server `:933`), `GITFORGE_ARTIFACT_ROOT` (api `:50`, ci `:217`), `HTTP_PORT`/`SSH_PORT` (git-server `:68,:72`); secrets `JWT_SECRET`/`JWT_SECRET_FILE`, `GITFORGE_CI_TRIGGER_TOKEN` must never be printed | see cited lines |

**Negative contracts that must be preserved by any future code work**
(re-anchored at the pin; they pin today's failure mode as tests):

- `crates/gitforge-api/tests/ci_routes.rs:1137-1190` —
  `pipeline_run_trigger_resolves_revisions_in_repository_storage`: missing
  storage → 500 `storage_unavailable`; unknown revision → 400
  `unknown_revision`; CI unconfigured → 503 `ci_unavailable`; authorization
  applies first → 403.
- `crates/gitforge-api/tests/ci_routes.rs:1192-1215` —
  `pipeline_run_trigger_validates_pipeline_and_id`: unknown pipeline → 404;
  malformed id → 400 `invalid_id`.
- `services/git-server/tests/git_http_edges.rs:353-412` —
  `test_unknown_and_unbacked_repositories_are_not_served`: unknown repo →
  404 (`:359-382`; info/refs and both service endpoints); a row that
  exists in the DB with no provisioned storage → 404, not 500
  (`:384-409`).

## 7. Standing rules (all phases, no exceptions)

1. **No direct SQL `UPDATE`/`DELETE`/`INSERT` against the live database.**
   Live DB access is read-only; all analysis runs on the Phase 0 snapshot.
2. **No deleting old rows or old directories/objects** — originals and
   candidate copies are preserved until recovery is accepted (G6).
3. **No symlink or path rewrite** to make an absent `git_path` resolve.
4. **No fresh repo registration / re-push** as a "fix". Registration may
   only be the executed outcome of an approved Phase 2 disposition (b)
   with parity evidence.
5. **No pipeline trigger before G3** (parity accepted) **and** explicit
   operator approval.
6. **Nothing is written into a live registered path before G3**, and then
   only by the staged procedure in §8 (Phase 4).
7. **A 200 health check never constitutes recovery progress or success.**
8. Every phase produces durable evidence (receipt file + hashes) *before*
   the phase closes.

## 8. Phased procedure

**Owner legend** (no invented names; roles are the named owners):

- **Operator** — human approval authority (M. Kinney). Sole authority for
  approvals at G2/G3/G5/G6 and anything touching the live host.
- **Recovery Agent** — the executing agent/session for this handoff.
- **Verifier** — an independent second agent/session producing dual
  evidence; must not share the Recovery Agent's conclusions as input.

Each phase ends with a **binary completion criterion** — it is done or it
is not.

---

### Phase 0 — Freeze, snapshot, environment capture

**Owner:** Recovery Agent (Operator informed). **Gate:** G0.

1. **Declare the freeze:** no pipeline triggers, no repo registration, no
   service restarts, no file operations under
   `/home/mkinney/work/gitforge-audit-20260822/` (or any other candidate
   storage root), no scratch-GC touching candidate roots. Register a lock
   on the retired root with `agent-locks acquire` so concurrent agents and
   GC passes stay out.
2. **Capture service environment without exposing secrets.** For each
   `gitforge*` unit, record the env-file path and the **names** of all
   variables; record values only for the non-secret set needed below
   (`DATABASE_URL`, `GITFORGE_DATABASE_URL`, `GIT_ROOT`,
   `GITFORGE_ARTIFACT_ROOT`, `HTTP_PORT`, `SSH_PORT`, `PORT`).

   ```bash
   systemctl list-units 'gitforge*' --no-legend
   for u in gitforge-api gitforge-ci gitforge-git-server gitforge-runner; do
     systemctl show "$u" -p EnvironmentFiles -p MainPID
     tr '\0' '\n' < "/proc/$(systemctl show -p MainPID --value "$u")/environ" \
       | cut -d= -f1 | sort          # variable NAMES only
   done
   # Then capture the named non-secret values individually. Never echo
   # JWT_SECRET*, *TOKEN*, *PASSWORD* values; record presence only.
   ```
3. **Fresh address discovery (mandatory, subnet changed):** current IPv4 of
   the Fedora host (`ip -4 addr show`), then fresh
   `curl -fsS http://<current-addr>:<port>/health` for api/ci/git-server
   with the ports captured in step 2. Record the address + timestamp in the
   receipt. Do **not** reuse `192.168.1.202`.
4. **Consistent SQLite snapshots** (WAL databases: never `cp` the live
   `.db`/`.db-wal`/`.db-shm` triple):

   ```bash
   STAGE=/var/tmp/gitforge-aegis-recovery-20261011   # NVMe per scratch policy
   install -d "$STAGE/db" "$STAGE/receipts"
   sqlite3 "$API_DB_PATH" ".backup '$STAGE/db/api-snapshot-$(date +%Y%m%dT%H%M%S).db'"
   # Then snapshot the CI scheduler DB the same way: it is a separate
   # database, configured by the CI unit's GITFORGE_DATABASE_URL
   # (services/ci/src/main.rs:188) — NOT DATABASE_URL.
   sqlite3 "$SNAP" 'PRAGMA integrity_check;'     # must print: ok
   sha256sum "$STAGE"/db/*.db | tee "$STAGE/receipts/db-snapshot.sha256"
   ```

   `API_DB_PATH` comes from the API unit's `DATABASE_URL`
   (default `sqlite:/gitforge.db`, `services/api/src/main.rs:182`). The
   CI scheduler's DB path comes from the **CI unit's**
   `GITFORGE_DATABASE_URL` (`services/ci/src/main.rs:188`) — a different
   variable, pointing at a different database; both must be snapshotted.
5. **Record baseline state hashes:** sha256 of the snapshot(s), sizes, and
   a read-only dump of the affected rows from the **snapshot** (§8 Phase 1)
   — this is the preserved "before" evidence.

**G0 (binary):** freeze declared + lock registered; env names captured
(secrets absent); fresh address + fresh 200s recorded with timestamp;
`integrity_check` = `ok` on every snapshot; sha256 receipts written.
Any miss → G0 not met; stop and report.

---

### Phase 1 — Read-only exhaustive census

**Owner:** Recovery Agent; Verifier re-runs the classification read-only.
**Gate:** G1. Everything here is read-only against the snapshot and the
filesystem.

1. From the **snapshot**, dump every repository row and every pipeline row
   to the census files, including the classifier input consumed by step 2:

   ```bash
   sqlite3 -header -csv "$SNAP" \
     "SELECT id,name,git_path,owner_id,visibility,required_checks,deny_non_fast_forward,created_at,updated_at FROM repositories ORDER BY name;" \
     > "$STAGE/receipts/census-repos.csv"
   sqlite3 -header -csv "$SNAP" \
     "SELECT id,repo_id,name,trigger_type,created_at FROM pipelines ORDER BY repo_id;" \
     > "$STAGE/receipts/census-pipelines.csv"
   # pipelines.config per row into a separate file (needed at Phase 3)

   # Classifier input for step 2, exactly the pipe-delimited id|name|path
   # it consumes — derived from the Phase 0 `.backup` snapshot (read-only)
   # with `sqlite3 -separator '|'`. Never read, cat, or parse the live
   # database file. A `|` inside git_path is harmless: it stays inside
   # `path`, the final read field.
   sqlite3 -separator '|' "$SNAP" \
     "SELECT id,name,git_path FROM repositories ORDER BY name;" \
     > "$STAGE/receipts/repos.txt"
   ```
2. Classify **every** repository row. Taxonomy — mutually exclusive, one
   class per row, decided by the **first matching rule in this order**:
   `missing` (path does not exist) / `non_directory` (exists, not a
   directory) / `present_not_openable` (directory, not a git repo) /
   `non_bare` (openable, not bare — decided **before** any ref check: a
   non-bare repo with refs is `non_bare`, never `refs_available`) /
   `openable_no_refs` (bare, zero refs) / `refs_available` (bare, ≥1
   ref). Plus the inverse class `orphan` for directories under any
   storage root with no DB row.

   ```bash
   while IFS='|' read -r id name path; do
     st=missing
     if   [ -e "$path" ] && [ ! -d "$path" ]; then st=non_directory
     elif [ -d "$path" ]; then
       if ! git -C "$path" rev-parse --git-dir >/dev/null 2>&1; then
         st=present_not_openable
       elif ! git -C "$path" rev-parse --is-bare-repository | grep -q true; then
         st=non_bare                  # precedence: non-bare beats ref state
       elif [ "$(git -C "$path" show-ref 2>/dev/null | wc -l)" -eq 0 ]; then
         st=openable_no_refs
       else
         st=refs_available
       fi
     fi
     printf '%s|%s|%s|%s\n' "$id" "$name" "$path" "$st"
   done < "$STAGE/receipts/repos.txt" | tee "$STAGE/receipts/census-classification.txt"
   ```

   **Verifier protocol:** the Verifier re-executes **this exact
   script** (copied into the receipt with its sha256), against the same
   snapshot and `repos.txt` — not a re-derived or reformatted variant.
   G1 agreement is a double-execution check of one classifier, not a
   comparison of two.
3. **Orphan scan:** list `mindepth 1 -maxdepth 1 -type d` under each known
   storage root (the captured `GIT_ROOT`, plus every parent directory of
   any `git_path` value in the census); set-difference against the DB UUID
   set (`comm`). Orphans are preserved in place — never deleted here.
4. **Candidate-copy search for every `missing` repo** (record negatives
   too — "searched, not found" is evidence):
   - `find` for `*.bundle`, and for directories named by UUID or repo
     name, across `/home/mkinney` and any other plausible roots.
   - NAS search **once, batched** (`/nas/Temp/tmp`, `/nas/Temp/archive`
     + `MANIFEST.tsv`, `*.bundle`) — the JBOD is a spinner; one sweep, not
     per-file probing.
   - Worktrees of Aegis source checkouts (`git worktree list`,
     `git for-each-ref`) — as **candidate copies only**. Never assume a
     source checkout has equivalent refs to the platform repo (CI-era
     tags/refs may exist only in platform storage, and vice versa).
   - GitHub mirror: once the org/repo mapping is confirmed with the
     Operator, `git ls-remote <mirror-url>` and record full output.
5. Consolidate: one census summary table (repos × classification ×
   candidates) in the receipt directory.

**G1 (binary):** 100% of repository rows classified; all storage roots'
orphans enumerated; every `missing` repo has a recorded candidate-search
verdict (found-with-hashes, or not-found-after-listed-search). Verifier
re-runs the same classifier script per the step-2 protocol and agrees.
Spot-checks, sampling, or "probably fine" do not satisfy G1.

---

### Phase 2 — Per-repo decision (Operator-approved)

**Owner:** Recovery Agent proposes; **Operator disposes**; Verifier checks
evidence completeness. **Gate:** G2.

For every non-`refs_available` row, exactly one disposition:

- **(a) restore-from-verified-copy** — a candidate copy exists; it is
  hash-recorded, its provenance chain is documented (where it came from,
  when, by whom), and it passes `git fsck` (§ Phase 3). Honesty rule:
  when no external ground truth exists, "verified" means *internally
  consistent + provenance-documented*, and the receipt says exactly that —
  not "proven identical to the lost original".
- **(b) re-register** — only after **proof** that nothing unique would be
  lost (e.g. the repo is an empty mirror of an upstream that verifiably
  still exists, or the census shows zero refs and the Operator accepts the
  loss). Requires explicit Operator approval recorded in the table.
- **(c) preserve row, mark blocked** — no acceptable copy exists; the row
  and any related objects stay untouched; the repo stays in the API with
  its current (failing) storage contract.

**Staging discipline (all restores):** copies are staged under
`$STAGE/stage/<uuid>/` and parity-proven there. **Nothing moves into a
live registered path before G3 + the Phase 4 procedure.** Restores target
the **originally registered path** (so the DB row remains valid with zero
DB writes); migrating storage roots is a separate, future, approved task —
explicitly out of scope here.

**G2 (binary):** a decision table exists with zero undecided rows; every
row carries evidence links and (for a/b) Operator approval. Rollback:
trivially — no action has been taken yet; discard the proposal table.

---

### Phase 3 — Parity proof (read-only until approved)

**Owner:** Recovery Agent produces; Verifier dual-checks. **Gate:** G3.
Commands here run against staged copies and existing readable repos only —
all read-only with respect to live services and originals.

Per repo slated for restore, record into a parity dossier:

```bash
R=$STAGE/stage/<uuid>
git -C "$R" fsck --full --no-progress                     # must be clean
git -C "$R" count-objects -v
git -C "$R" for-each-ref --format='%(refname) %(objectname)'   # ALL refs, incl. hidden
git -C "$R" symbolic-ref HEAD                             # default branch
git -C "$R" tag --list
git -C "$R" config --list --local   # includes receive.denyNonFastForwards policy
```

Plus, reconciled against the snapshot DB row/pipeline row:

- `required_checks` and `deny_non_fast_forward` (DB columns,
  `crates/gitforge-db/src/queries.rs:131-137`) vs the repo's local config.
- Pipeline `config` from the DB snapshot vs
  `git -C "$R" show <default-tip>:<pipeline-file>` (e.g. `.gitforge.yml`).
- Receipt copies: `git bundle create "$STAGE/bundles/<uuid>.bundle" --all`
  (inside the staged repo; writes only to staging), then
  `git bundle verify` + `sha256sum` of the bundle.
- Where a mirror/upstream exists: `git ls-remote <url>` output attached,
  and any deltas between mirror refs and staged refs **listed explicitly**.

**G3 (binary):** for every restore-slate repo — `fsck` clean, ref/object
inventory recorded, default branch + tags + hidden refs enumerated, config
policy reconciled, bundle receipt hashed, dossier dual-signed by Verifier.
Any `fsck` error or unexplained ref delta → abort that repo to disposition
(c) and escalate.

---

### Phase 4 — Canary restore, then one-at-a-time

**Owner:** Operator approves; Recovery Agent executes; Verifier observes.
**Gate:** G4 (canary) / G5 (fleet).

1. **Canary selection:** the least-critical restore-slate repo (smallest
   history, lowest evidence value), not automatically `aegis`.
2. **Placement** (reversible, no DB writes, no in-place overwrite):

   ```bash
   T=<registered-parent-of-git_path>            # e.g. .../runtime-repos
   cp -a "$STAGE/stage/<uuid>" "$T/.<uuid>.incoming"   # copy onto target fs
   test ! -e "$T/<uuid>"   # HARD GATE: destination must not exist — if it
                           # did, mv would nest the staged copy INSIDE it
                           # (as $T/<uuid>/.<uuid>.incoming), not replace it
   mv "$T/.<uuid>.incoming" "$T/<uuid>"                # rename: atomic within target fs
   git -C "$T/<uuid>" fsck --full --no-progress        # post-move re-proof
   ```

   The staged copy stays in `$STAGE` until G6 (dual evidence). Rollback
   at any instant, two hops: first
   `mv "$T/<uuid>" "$T/.<uuid>.rolled-back"` — a rename within the
   target filesystem (atomic), after which the registered path has
   returned to absent, exactly the pre-incident state. Only then move
   `.rolled-back` into `$STAGE/rollback/<uuid>` for retention: that
   second hop crosses filesystems (`$T` → `/var/tmp`), so it is
   copy-plus-unlink, **not atomic** — it is interruptible and touches
   only the already-unregistered copy; re-verify it (`fsck` + hash vs
   dossier) once it arrives.
3. **Canary probes, in order; stop at the first unexpected response:**
   1. Authenticated API lookup of the repo returns the row (owner username
      resolved from `owner_id` in the snapshot; endpoint per `docs/API.md`).
   2. HTTP Git: `git ls-remote http://<addr>:<git-port>/<owner>/<name>.git`
      then a full `git clone --no-local` to a throwaway dir; ref counts
      match the dossier.
   3. SSH Git: `git ls-remote ssh://git@<addr>:<ssh-port>/<owner>/<name>.git`.
   4. **Only now** the trigger-acceptance probe:
      `POST /api/pipelines/<pipeline-uuid>/runs` with the dossier's default
      tip — expect `202` + `pipeline_run_id`. This runs real CI; it is
      gated on G3 + Operator approval (rule §7.5).
   5. The run reaches a terminal state; logs are retrievable; job verdicts
      recorded.
4. **Remaining repos one at a time**, full probe set each, no batching.
5. **Abort criteria (any one stops everything):** any probe response
   differs from expectation; ref/object counts differ from the dossier;
   any error requiring a live-DB write to "fix"; service instability
   post-placement.

**G4 (binary):** canary receipt shows all five probes with expected
responses and attached outputs. **G5 (binary):** every restore-slate repo
has the same full-probe receipt.

---

### Phase 5 — Durable receipts, verdict, cleanup

**Owner:** Recovery Agent drafts; Operator signs. **Gate:** G6.

1. File dated receipts (commands, hashes, probe outputs, decision table)
   in the repo as a `docs/audits/` or `docs/handoffs/` receipt, and append
   the verdict entry to the planning ledger with evidence citations
   (ledger entries are append-with-verdict; cite hashes/paths, not
   recollections).
2. Verdict per repo: `restored` / `re-registered` / `blocked-preserved-row`
   — with the evidence chain. **Recovery is not "complete" because health
   endpoints return 200** (§2.3); completeness is the receipt set.
3. Cleanup only after the Operator documents retention: snapshots, bundles,
   and staged copies are retained per the retention decision (default
   proposal: keep until one full successful backup cycle + 30 days), then
   removed by an enumerated, reviewed deletion pass — never an ad-hoc
   `rm -rf`.
4. Follow-up owed regardless of verdict: the preventive engineering item
   (§10) and the pending self-hosted pipeline status (§14).

**G6 (binary):** receipts filed and linked from the ledger; verdicts
recorded; retention documented; cleanup either done-with-manifest or
explicitly deferred with a dated owner.

## 9. Gates and standing abort triggers

| Gate | Meaning | Binary criterion |
|---|---|---|
| G0 | Freeze + snapshots | §8 Phase 0 criterion met, hashes recorded |
| G1 | Census complete | 100% rows + orphans classified; candidate verdicts recorded; Verifier agrees |
| G2 | Decisions made | Decision table complete, Operator-approved |
| G3 | Parity proven | Dossier per restore-slate repo, fsck clean, dual-signed |
| G4 | Canary proven | All five probes green with outputs |
| G5 | Fleet restored | Full-probe receipt per restored repo |
| G6 | Closure | Receipts in ledger, retention documented |

**Abort immediately (stop-all, preserve state, escalate to Operator) on:**
any fsck failure on a candidate whose original is gone; any hash mismatch
between dual evidence copies; any probe response deviation in Phase 4;
any discovered mutation of original/registered paths during the campaign;
any step that would require live-DB writes; any ref-count delta vs dossier.

## 10. Preventive engineering follow-up (separate code task)

**Keep this strictly separate from the data-recovery campaign above**, and
separate from the optional `storage_unavailable` 500-contract /
path-redaction changes around
`crates/gitforge-api/src/routes/ci.rs:508-518` (whose negative contracts in
`crates/gitforge-api/tests/ci_routes.rs:1137-1215` must be preserved).

**Feature:** report-only registration↔storage reconciliation, living in
**`services/git-server`** (the service that owns `GIT_ROOT`,
`services/git-server/src/main.rs:930-934`):

- Enumerate rows via `RepoQueries::list`
  (`crates/gitforge-db/src/queries.rs:429-441`) — the same hydration this
  incident exploited (no path validation).
- Classify each repo path: `missing` / `malformed` (exists, not a
  directory) / `non_bare` / `ok` — with the same precedence as the
  Phase 1 census: `non_bare` is decided before any ref check, so a
  non-bare repo with refs reports `non_bare`, never `ok`; enumerate
  orphan directories under `GIT_ROOT` vs the row set (layout per
  `crates/gitforge-core/src/storage.rs:85-87`).
- Emit **metrics** (counters per classification) and a **structured,
  redacted `tracing` warning + serializable receipt** per pass — receipt
  style follows the sandbox reconciler precedent
  (`crates/gitforge-sandbox/src/reconciler.rs:1-30`): counts and ids, never
  logs, tokens, or env values.
- **By default: no deletion, no auto-repair (no `git_path` rewrite, no
  directory creation), and no change to `/health` readiness.** Census-only,
  mirroring the sandbox reconciler's fail-closed default.
- **Tests (required):** unit tests for the classifier (each taxonomy class,
  table-driven); an integration test proving that with N registered repos
  and exactly one missing storage dir, the report flags exactly that repo
  **while the other N−1 repos still serve** info/refs and clone normally —
  the property that was missing when this incident surfaced.

This is hardening *after* recovery; it does not recover anything.

## 11. Milestones, dependencies, acceptance evidence

| Milestone | Depends on | Acceptance evidence | Owner |
|---|---|---|---|
| M0 freeze+snapshot | — | G0 receipts (hashes, env names, fresh address/health) | Recovery Agent |
| M1 census | M0 | G1 census files + classification + candidate verdicts, dual-run | Recovery Agent + Verifier |
| M2 decision table | M1 | G2 table, Operator approval recorded | Operator |
| M3 parity dossiers | M2 | G3 dossiers, fsck clean, bundle hashes, dual-signed | Recovery Agent + Verifier |
| M4 canary | M3 + Operator approval | G4 five-probe receipt | Recovery Agent |
| M5 fleet restore | G4 | G5 receipts, one per repo | Recovery Agent |
| M6 closure | G5 (or all-blocked) | G6 ledger entry + retention record | Operator |
| M7 (separate) preventive reconciler | independent of M0–M6 | merged code + the §10 tests | engineering task, own branch/PR |

## 12. Rollback

- **Phases 0–3:** nothing has touched live state; rollback is discarding
  staging artifacts (after the retention decision).
- **Phase 4/5 placement:** `mv` the placed directory back out of the
  registered path (see §8 Phase 4 step 2) — the first hop is the rename
  into `$T/.<uuid>.rolled-back`, atomic within the target filesystem,
  and it alone restores the pre-incident (absent) registered state; the
  subsequent move into `$STAGE/rollback/` may cross filesystems and is
  copy-plus-unlink, not atomic (re-verify on arrival). DB rows were
  never modified; services were never restarted; the staged verified
  copy remains intact for a later retry.
- **No rollback ever requires SQL, because no SQL was ever run against the
  live DB.**

## 13. Owners

- **Operator (M. Kinney):** approvals at G2/G3/G4-entry/G5/G6; retention
  decision; sole authority for live-host actions outside this handoff.
- **Recovery Agent:** executes Phases 0–5; owns receipts.
- **Verifier:** independent dual evidence at G1/G3/G4; must not consume
  the Recovery Agent's outputs as its own inputs.

## 14. Honest limitations of this handoff

1. **No runtime observation herein was independently reproduced by the
   author.** Everything in §2 is quoted from the 2026-10-11 session and is
   **stale**: the subnet changed after it was collected. Every address,
   health result, and journal count must be remeasured (Phase 0) before
   use. Do not hardcode `192.168.1.202`.
2. **Source anchors are pinned to `8431f1170608bc8fc86862777e2619bdfa5d8387`
   (GitHub `main`), not to the old local branch.** One anchor already
   drifted from the earlier briefing (`get_git_root` is `:930-934` at the
   pin). Re-anchor before citing in execution receipts.
3. **No path state, restore evidence, or test result has been fabricated
   or assumed.** Where this document says a copy "may exist", that is an
   open question (§4), not a finding. If Phase 1 finds nothing, the honest
   verdict is disposition (c) for the affected repos and an evidence-loss
   record — not a search for a palatable story.
4. The DB path/values, GIT_ROOT value, port values, and owner usernames on
   the live host are unknown here; Phase 0 captures them.
5. **GitForge's self-hosted pipeline is a *pending* follow-up for this
   branch** — this PR is docs-only and no GitForge pipeline result is
   claimed as passed. A green GitHub check is a mirror, not the gate
   (repo `CLAUDE.md`).
6. This handoff intentionally performs **zero** mutations. Its only output
   is this document; every action it describes waits on its gate.

## Appendix A — Read-only command quick reference

```bash
# Snapshot (writes only to staging)
sqlite3 "$DB" ".backup '$SNAP'" && sqlite3 "$SNAP" 'PRAGMA integrity_check;'

# Row dump (against the SNAPSHOT, never live)
sqlite3 -header -csv "$SNAP" "SELECT id,name,git_path,owner_id,visibility,required_checks,deny_non_fast_forward,created_at,updated_at FROM repositories ORDER BY name;"

# Per-path classification (read-only)
test -d "$p"; git -C "$p" rev-parse --git-dir; git -C "$p" rev-parse --is-bare-repository
git -C "$p" show-ref; git -C "$p" for-each-ref; git -C "$p" symbolic-ref HEAD
git -C "$p" fsck --full --no-progress; git -C "$p" config --list --local
git -C "$p" count-objects -v

# Candidate copies
git bundle verify "$B"; git ls-remote <url>; find <roots> -name '*.bundle'

# Safe failure-mode probe (enqueues nothing — check precedes enqueue, ci.rs:508-518)
curl -s -X POST -H "Authorization: Bearer $TOK" -H 'Content-Type: application/json' \
  -d '{"ref":"HEAD"}' "http://<addr>:<port>/api/pipelines/<uuid>/runs"
# expected while storage is missing: 500 {"error":"storage_unavailable"}
```
