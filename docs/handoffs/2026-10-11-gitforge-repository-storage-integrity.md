# Handoff: GitForge fleet repository-storage integrity

Date: 2026-10-11

> **Currency fence (2026-10-11):** every observation below was made on
> 2026-10-11 through read-only SSH and `sqlite3` `mode=ro` access; it is not
> proof of deployed state at reading time. Fedora storage and DB state move
> daily; re-run the census (§3) before acting.

Priority: P1 — 15 of 21 registered repositories on the live Fedora GitForge
instance have no backing storage; any clone, fetch, or pipeline run against
them fails today.
Status: open — fleet census verified, **no recovery, restore, or live
service/DB mutation has occurred**.
Audience: PA operator (Fedora host owner); GitForge maintainer; per-repo
owners (mostly unidentified — see §12).

Scope of this handoff: documentation only, written in worktree
`/home/mkinney/worktrees/gitforge-storage-integrity-handoff-20261011`
(branch `codex/gitforge-storage-integrity-handoff-20261011`). Every Fedora
action during this investigation was read-only: SSH file inspection, one-level
directory listings, journal reads, and `sqlite3` opened as
`file:…audit.db?mode=ro`. No write, restore, push, clone into live storage,
config change, service change, or GitForge registration change was attempted,
on Fedora or locally. No credentials or env files were read; no `sudo` was
used; `/nas` was not recursively crawled (one-level name checks and a single
manifest grep only).

**Hard constraint carried through every phase below: no missing repository
may be blindly recreated or deleted.** Every disposition requires the
per-repo evidence of §7 and a recorded decision in §8 Phase 1.

## 1. Incident statement

The Fedora GitForge standalone instance (user units
`gitforge-{api,ci,git-server,runner}.service`, host alias `fedora`,
`mkinney@192.168.1.202`) serves SQLite DB
`/home/mkinney/work/gitforge-audit-20260822/audit.db` with storage root
`GIT_ROOT=%h/work/gitforge-audit-20260822/runtime-repos` (verified from
`systemctl --user cat gitforge-git-server.service` and
`gitforge-api.service` units, 2026-10-11). As of 2026-10-11:

- 21 rows exist in `repositories`; 15 recorded storage paths are absent;
  6 exist on disk.
- By owner: `codex-audit` (user `b4ada040-ffec-4748-aa91-20628810c368`,
  role developer) has 12 of 14 missing; `operator` (user
  `584fccb1-1de3-41b0-8b52-e4505a4cb2ff`, role admin) has 3 of 7 missing.
- 12 of the 15 missing paths are under `runtime-repos/`; the other 3 point
  outside the storage root entirely (`/var/lib/gitforge/…`, `/git/repos/…`),
  and neither prefix exists on the host.
- The 6 surviving directories all carry mtimes 2026-10-10 09:02 through
  2026-10-11 13:16 — i.e. they were (re)created for the current live stack;
  one of them (`control-center` under `codex-audit`) had a `running`
  pipeline at census time (2026-10-11T19:24:32Z).

This is the fleet-level ("F5 census") incident. The single-repo recovery of
`taskwizer-rust-cli` is separately planned and executed; this document is the
fleet control plane around it.

## 2. Relation to the TaskWizer handoff (no duplication)

The per-repo recovery runbook for repo `f18e9d03-af08-4603-b98d-9f76e58e3724`
(`taskwizer-rust-cli` under `codex-audit`) is canonical at:

- repo-of-record path:
  `/home/mkinney/worktrees/pa-taskwizer-storage-recovery-handoff-20261011/docs/handoffs/2026-10-11-taskwizer-gitforge-storage-recovery.md`
- branch `codex/taskwizer-storage-recovery-handoff-20261011`; introduced at
  commit `f62e0bf3` ("docs: add taskwizer storage recovery handoff"),
  hardened at `6bc96db6` ("docs: harden TaskWizer recovery runbook") —
  re-pin to that branch's tip before executing

That document owns: the TaskWizer source-of-truth decision tree (S0–S3), its
gated Phases 0–3, and its completion criteria. This handoff does **not**
repeat them. It cross-links them as the template for every Phase-2 "restore"
disposition (§8) and adopts its laws verbatim: durable-evidence law (judge
from DB rows, not transport impressions), never force-push, classify
401/403/404 before calling a transport healthy, never label an inaccessible
source as absent.

The TaskWizer recovery, even when complete, closes exactly one row. **The
F5 census is a fleet incident and is not closed by TaskWizer alone** — 14
other rows need their own evidence, dispositions, and receipts.

## 3. Verified census (2026-10-11, read-only — exact PA census reproduced)

Method (identical result class to PA evidence F5, independently re-run):

```bash
ssh fedora 'python3 -' <<'PY'
import os, sqlite3
con = sqlite3.connect(
  "file:/home/mkinney/work/gitforge-audit-20260822/audit.db?mode=ro", uri=True)
rows = con.execute("""
 SELECT r.id, r.name, u.username, r.visibility, r.git_path, r.created_at
 FROM repositories r JOIN users u ON u.id = r.owner_id
 ORDER BY u.username, r.created_at""").fetchall()
for r in rows:
    print(r[0], r[1], r[2], r[3], os.path.isdir(r[4]), r[4], sep="|")
PY
```

Result: **TOTAL=21 PRESENT=6 MISSING=15; `codex-audit` 12 of 14 missing;
`operator` 3 of 7 missing; repo `f18e9d03-…` among the missing** — the exact
census PA recorded (F5) was reproduced on 2026-10-11.

Inventory. "Exist" = `os.path.isdir(git_path)`. "Last run" = latest
`pipeline_runs` row (status@UTC, `commit_hash` where non-zero); a non-zero
hash proves storage existed at that instant (the run required a push/clone).

| # | Repository id | Name | Owner | Exist | Last run (status@UTC, hash) |
|---|---|---|---|---|---|
| 1 | `14c3024d-23db-4ca0-ac21-7aa7e9d7346e` | codex-trigger-fixture | codex-audit | **NO** (path `/var/lib/gitforge/codex-trigger-fixture.git`) | 2026-08-29T02:41Z failed `0e4f640b700b` |
| 2 | `9777ba1f-180d-4972-aa8b-454da2d17cf6` | disposable-canary-20260830 | codex-audit | **NO** | 2026-08-30T18:01Z failed `dc1f95b47590` |
| 3 | `30cd4d40-a1ba-4158-a0e2-38a652d656d7` | ci-loop-test | codex-audit | **NO** | 2026-09-23T05:07Z succeeded `e170ec6b2b38` |
| 4 | `6905d6ea-3c2a-4a0a-9434-edabc83837b5` | gitforge | codex-audit | YES (mtime 10-10 19:52) | 2026-10-11T04:18Z failed `1f0cc779a5a2` |
| 5 | `202ba6c6-0986-4675-8ec4-610dba62df31` | control-center | codex-audit | YES (mtime 10-11 13:16) | 2026-10-11T19:24Z **running** `bcd6ce3a4c20` |
| 6 | `9ec54026-0ec6-4626-a8dd-e6bccd2396ac` | aegis-platform-validation-20260830 | codex-audit | **NO** (path `/git/repos/aegis-platform-validation-20260830`) | 2026-08-31T02:03Z failed `5a5c742919c0` |
| 7 | `72d9f2a7-48d5-433b-ae72-5195989cbdb6` | aegis-platform-validation-20260831-r2 | codex-audit | **NO** (path `/git/repos/…-r2`) | no runs |
| 8 | `4c9fe735-d917-423e-91d9-b5ea1f89b361` | aegis-platform-validation-20260831-r3 | codex-audit | **NO** | no runs |
| 9 | `d1326cfb-e959-4e7d-bfbd-527eb2f97dcd` | aegis-platform-validation-20260831-r4 | codex-audit | **NO** | 2026-08-31T04:13Z failed `cc3394ad6dea` |
| 10 | `1e864116-6854-4028-b150-153b22995057` | amortyx | codex-audit | **NO** | 2026-10-09T09:13Z cancelled `6468abf89bc0` |
| 11 | `4eefae58-e76e-4e63-a876-5c4900504305` | Platform-Architecture | codex-audit | **NO** | 2026-10-09T04:23Z succeeded `dca8a3cc7857` |
| 12 | `db351fd7-b240-4e73-865e-0cb7d86e4ef8` | control-center-api-canary-20260831 | codex-audit | **NO** | 2026-08-31T16:31Z succeeded `cf645ff70076` |
| 13 | `f18e9d03-af08-4603-b98d-9f76e58e3724` | taskwizer-rust-cli | codex-audit | **NO** | 2026-09-09T10:18Z failed `0000…`; last real 2026-08-31T20:15Z `db46d286d9de` — **owned by the TaskWizer handoff (§2)** |
| 14 | `26441efa-decf-408e-a27b-7eb90cb9e269` | dsc | codex-audit | **NO** | 2026-09-14T14:29Z succeeded `4b216666a51e` |
| 15 | `b09d49f8-f1ba-4045-8305-3ab7c0a071ca` | aegis | operator | **NO** | 2026-10-07T19:45Z failed `35ab3581f6db` |
| 16 | `3b0be444-fd6c-4335-bd4d-f1aa66af4751` | Control-Center | operator | **NO** | 2026-10-08T15:57Z succeeded `7d432d2251ee` |
| 17 | `0ddd4cdf-0337-4e14-b81f-62e93d4111f3` | amortyx-nightly | operator | YES (mtime 10-10 09:02) | 2026-10-11T01:00Z failed `92b9805b7cbe` |
| 18 | `dd8530c0-40db-4028-8e97-810f542a76b7` | Harness | operator | **NO** | no runs |
| 19 | `ac780aad-2fcb-41a1-b4df-f149ae7bb92b` | amortyx | operator | YES (mtime 10-10 23:57) | 2026-10-11T06:47Z failed `219339207ad5` |
| 20 | `4509402f-03c1-402a-be78-eca700e5d66b` | gitforge | operator | YES (mtime 10-11 03:22) | 2026-10-11T19:28Z failed `108ee59c661a` |
| 21 | `eccda35c-2465-4ba2-957f-de58047fc680` | Platform-Architecture | operator | YES (mtime 10-10 10:36) | 2026-10-10T17:12Z succeeded `ffdfa47d8cec` |

Missing-path taxonomy: rows 2,3,8,9,10,11,12,13,14,15,16,18 → absent paths
under `runtime-repos/<repo-id>` (parent exists); rows 1,6,7 → recorded paths
outside the storage root whose prefixes (`/var/lib/gitforge`, `/git`) do not
exist on the host at all. All rows are `visibility=private` except row 2
(`public`).

## 4. Evidence classes

### Observed (2026-10-11, commands cited)

- O1 — Census: §3 (sqlite `mode=ro` + `os.path.isdir`), 15/21 missing,
  12/14 `codex-audit`, TaskWizer row missing. Matches PA F5 exactly.
- O2 — `systemctl --user cat gitforge-git-server.service` /
  `gitforge-api.service`: `DATABASE_URL=sqlite:%h/work/gitforge-audit-20260822/audit.db`,
  `GIT_ROOT=%h/work/gitforge-audit-20260822/runtime-repos`,
  `ExecStartPre=/usr/bin/test -r …audit.db`; ports 42782 (HTTP), 42022 (SSH).
  Same DB and root as PA F3.
- O3 — Engine identity: `readlink -f ~/work/gitforge-current` →
  `/home/mkinney/work/validation/releases/gitforge-4d72efc2-20261010`
  (deployed 2026-10-10 12:44 local, matching PA F10).
- O4 — Surviving storage: `ls -la runtime-repos/` shows exactly the 6 "YES"
  rows, mtimes 2026-10-10 09:02 … 2026-10-11 13:16; parent dir mtime
  2026-10-10 19:52. No dot-prefixed or stray entries.
- O5 — Filesystem: `findmnt -no FSTYPE,SOURCE /home` → `btrfs
  /dev/nvme1n1p3[/home]`; `stat -c '%d %n'` → device `49` for both
  `runtime-repos` and `gitforge-audit-20260822` (same device; §9).
- O6 — Journal (retained window now begins **2026-10-09**, earlier than PA
  F9's 2026-10-10 12:44 start): `journalctl --user -u gitforge-git-server.service`
  first retained line 2026-10-09T06:00Z; `gitforge-api.service` systemd
  start lines from 2026-10-09T16:20Z. A grep over 2026-10-09→11 for
  `delete|remove|RepoDeleted|purge|husk|runtime-repos` across
  git-server/api/ci units found **no** storage deletions — every "removed"
  hit is CI run-workspace cleanup under
  `…/workspaces/<run_id>` (e.g. `ci: removed run workspace run_id=…`),
  a different directory tree from `runtime-repos`.
- O7 — No backups exist yet: `ls /home/mkinney/backups/gitforge` → ENOENT
  (the PA Phase-0 optional DB snapshot has not been taken). `~/.local/share/Trash/files`
  contains no repo-id or repo-name entries. `/nas/Temp/archive/MANIFEST.tsv`
  exists (358 bytes) and has zero entries matching any of the 21 repo names.
- O8 — NAS source candidates (`test -e /nas/Temp/repos/<name>`, one level):
  present for `taskwizer-rust-cli`, `aegis`, `dsc`, `Platform-Architecture`,
  `Amortyx`, `Control-Center`, `GitForge`; absent for `Harness`,
  `ci-loop-test`, `control-center-api-canary` (and the canary/fixture names
  generally).
- O9 — NAS candidate remotes (`git remote get-url origin` + `rev-parse HEAD`,
  bounded, no crawl): `aegis`→`github.com/aliasfoxkde/aegis.git` @
  `370e5679d489`; `dsc`→`github.com/aliasfoxkde/dsc.git` @ `04dc922431dc`;
  `Platform-Architecture`→`github.com/WizerAI/Platform-Architecture.git` @
  `242d6f9b1dad` (branch `codex/fedora-load-audit-20261007`); `Amortyx`→
  `github.com/aliasfoxkde/Amortyx.git` @ `219339207ad5`; `Control-Center`→
  `github.com/aliasfoxkde/Control-Center.git` @ `07db7d0c2aeb`;
  `GitForge`→`github.com/aliasfoxkde/GitForge.git` @ `2a3ef128798f`;
  `taskwizer-rust-cli` @ `6e005c62b520` (the PA S2 witness).
- O10 — Cross-namespace content bridge: NAS `Amortyx` HEAD `219339207ad5`
  equals the **live** `operator/amortyx` row's latest run commit hash
  (2026-10-11T06:47Z, row 19) — the NAS clone is current with that
  project's live content. Live same-name rows exist under `operator` for
  `amortyx`, `gitforge`, `Platform-Architecture` (rows 19–21).

### Inferred (labelled, not directly observed)

- I1 — The removal of the 12 `runtime-repos` directories was an out-of-band
  cleanup during the 2026-10-10 live-stack recreation (survivor mtimes O4,
  engine deploy O3, PA's recorded operator "husk" decision), **not** an
  engine action: no code path removes storage while leaving the row (§5) and
  the retained journal covering Oct 09–11 shows no such deletion (O6).
  Actor, exact command, and exact time remain unknown.
- I2 — The 3 out-of-root rows (1, 6, 7) were never backed by this host's
  storage root: current code derives `git_path` exclusively as
  `<root>/<uuid>` at create time (crates/gitforge-core/src/repo.rs:88-97,
  crates/gitforge-api/src/routes/repo.rs:252-270), so these paths cannot
  have been produced by the current engine; they are legacy/manual rows.
  Serving would resolve `runtime-repos/<repo-id>` for them, which is also
  absent (O4 shows only the 6 survivor dirs).
- I3 — Per-row existence brackets from last real-hash runs (a run with a
  non-zero hash required a push/clone, hence storage): rows 11 (≥2026-10-09T04:23Z),
  10 (≥2026-10-09T09:13Z), 16 (≥2026-10-08T15:57Z), 15 (≥2026-10-07T19:45Z),
  3 (≥2026-09-23), 14 (≥2026-09-14), 13 (≥2026-08-31T20:15Z, PA F6/I3), and
  the Aug-29–31 fixtures/canaries. Rows 7, 8, 18 have **no runs ever** — DB
  evidence cannot prove they ever had storage.
- I4 — Remediation priority ordering follows blast radius: rows 10, 11, 15,
  16 had hundreds of runs and late activity (regression surfaces); rows
  7, 8, 18 have zero runs and are likely unused, but "likely unused" is an
  inference, never a deletion authority.

### Unknown

- U1 — Exact time/actor/command of the removals (journal retention begins
  2026-10-09; the recreation window operator actions were not logged).
- U2 — Whether root-managed btrfs/snapper snapshots of `/home` exist on
  Fedora (needs `sudo snapper list` / `sudo btrfs subvolume list /home`;
  not attempted — no sudo in this session). Applies fleet-wide, not just
  TaskWizer.
- U3 — Whether any of rows 1, 6, 7 ever had storage anywhere (out-of-root
  recorded paths; provenance of those rows unknown — I2).
- U4 — Owners of the projects behind rows 1–12, 14–16, 18 (only
  `taskwizer-rust-cli` has a recorded upstream owner, `aliasfoxkde`, per the
  TaskWizer handoff). GitHub-side existence/ancestry for the §7 candidates
  was not probed this session.
- U5 — Whether any pushed-but-never-cloned objects exist only in lost
  Fedora storage (the TaskWizer `db46d286d9de` problem generalized): for
  each row, commits between the last DB-recorded hash and the recovery
  source tip must be reconciled during Phase 1.

## 5. Root-cause investigation: how storage can and cannot disappear

Code facts (this worktree, HEAD `0fd54f7`; the deployed engine is bundle
`gitforge-4d72efc2-20261010` — same lineage, lines re-verify at that SHA
before citing in a code change):

- Serving resolves per request: owner/name → row via
  `RepoQueries::get_by_owner_and_name`
  (services/git-server/src/main.rs:237-258, crates/gitforge-db/src/queries.rs:444),
  then storage presence at `<GIT_ROOT>/<repo_id>`
  (crates/gitforge-core/src/storage.rs:85-92 — `repo_path` is
  `root.join(repo_id)`, `exists` is a metadata check); absent storage logs
  `repository not found in storage` and returns 404
  (services/git-server/src/main.rs:300-307) — the exact F13 symptom class.
  The DB `git_path` column is recorded metadata; serving reconstructs the
  path, so a staged sibling directory can never be resolved by accident.
- The engine's only repo-storage delete primitive is
  `FileStorageBackend::delete` (`remove_dir_all`,
  crates/gitforge-core/src/storage.rs:109-120). Its only live caller is the
  create-rollback path in `create_repo`
  (crates/gitforge-api/src/routes/repo.rs:286) — it runs when the DB insert
  just failed for storage created moments earlier. There is also a second
  `StorageBackend::delete` caller, `RepoService::delete`
  (crates/gitforge-core/src/repo.rs:124-136), which removes storage without
  a DB interaction; it is currently dead code (`let _repo_service = …` in
  services/git-server/src/main.rs:106), but could create this husk shape if
  a future caller wires it into lifecycle code.
- The user-facing delete route `DELETE /api/repos/{id}`
  (crates/gitforge-api/src/routes/repo.rs:360-417) deletes **only the DB
  row** (`RepoQueries::delete`, crates/gitforge-db/src/queries.rs:400) and
  never touches storage — the inverse hazard (orphaned storage), and it
  cannot produce this incident's shape (row present, storage absent).
- Row deletion is FK-gated in schema: `pipelines`
  (crates/gitforge-db/src/connection.rs:281), `pipeline_runs` (:316), and
  `pipeline_trigger_requests` (:359) reference `repositories(id)`, but the
  API path is not blocked. `RepoQueries::delete`
  (crates/gitforge-db/src/queries.rs:400-426) manually cascades dependent
  history before deleting the row in one `BEGIN IMMEDIATE` transaction, by
  design. `review_runs` at `connection.rs:686` uses `ON DELETE SET NULL`;
  `jobs` at `:509` references `pipeline_runs`, not `repositories`. Thus rows
  with runs can be retired through the API (row and history), which would
  orphan storage because `delete_repo` never touches it. What no live engine
  path does is remove storage while keeping the row; that observed husk
  shape still indicates an out-of-band removal, absent further evidence.

Conclusion (matches and extends PA I1): **no engine code path removes
registered storage while leaving the row**, and the retained journal shows
no engine-driven deletion in the Oct 09–11 window (O6). The 12 missing
`runtime-repos` directories were removed out-of-band, consistent with the
2026-10-10 stack-recreation cleanup; the mechanism and actor are U1. The 3
out-of-root rows are a different class (I2): legacy registrations whose
paths the current engine could never have written.

## 6. Prevention controls (proposals — none shipped)

- P1 — **Integrity reconciliation, scheduled and observable.** Today a
  missing repo is visible only as a 404 when addressed
  (`repository not found in storage`, services/git-server/src/main.rs:302)
  — silence until poked. Add a read-only reconcile (21-line census of §3)
  to `scripts/gitforge-status`/monitoring with a
  `gitforge_repo_storage_missing` count; alert on any increase.
- P2 — **Lifecycle state instead of husking.** The only sanctioned way to
  retire a repo should be an explicit operator state (`retired`/`husked`)
  that (a) flips the row state, (b) archives then removes storage under a
  receipt, and (c) makes the row answer 410-gone rather than 404. Ad-hoc
  `rm` of registered storage must become a detectable event (audit log
  line + the P1 metric). Also resolve the `delete_repo` asymmetry
  (§5): either delete storage after the row, or refuse when storage exists
  and the caller did not pass an explicit flag — decide with an ADR.
- P3 — **Standing DB snapshot cadence.** `/home/mkinney/backups/gitforge`
  does not exist (O7); the first snapshot should be the PA Phase-0 step-2
  sqlite-backup-API receipt, then a documented cadence in
  docs/RUNBOOK.md §Backup.
- P4 — **Journal retention ≥ cleanup horizon.** Retention currently starts
  2026-10-09 (O6) and already cannot see the removal window. Extend
  retention or emit a durable event for any storage-tree mutation so the
  next incident has a ledger, not a gap.
- P5 — **Schema guard for legacy rows.** Out-of-root `git_path` values
  (I2) are un-validatable; add a migration check/repair that flags any
  `git_path` not under the configured root, and surface them in the P1
  reconcile output rather than leaving them as silent 404s.

## 7. Per-repo preserve-first source discovery (bounded checks only)

Preserve-first rule: before any per-repo action, collect the receipts; a
source is chosen only after integrity gates (TaskWizer handoff Phase 0
steps 3–5 are the template: mirror, union refs, `fsck --full --strict`,
ancestry check against the row's last DB hash). Bounded checks already
executed this session (no crawling): DB receipts (§3), NAS one-level name
checks (O8), NAS candidate remote+HEAD (O9), archive manifest grep (O7),
trash listing (O7). Remaining bounded checks for execution phase: per-repo
GitHub probe (`gh repo view`/`merge-base --is-ancestor <last-db-hash> main`),
root-gated snapshot hunt (U2), and per-repo alternate-host checkouts.

| Row (§3 #) | Repo | DB last real hash | NAS candidate (O8/O9) | Live same-name row | Discovery status |
|---|---|---|---|---|---|
| 13 | taskwizer-rust-cli | `db46d286d9de` | yes @`6e005c62b520` | no | **fully planned — see TaskWizer handoff (§2)** |
| 10 | amortyx (codex) | `6468abf89bc0` | `/nas/Temp/repos/Amortyx` @`219339207ad5` | yes — operator row 19 (live, same hash as NAS HEAD) | strong candidate; verify ancestry vs `6468abf8…` |
| 11 | Platform-Architecture (codex) | `dca8a3cc7857` | `/nas/Temp/repos/Platform-Architecture` @`242d6f9b1dad` | yes — operator row 21 (live) | strong candidate; verify ancestry |
| 14 | dsc | `4b216666a51e` | `/nas/Temp/repos/dsc` @`04dc922431dc` | no | candidate; verify ancestry |
| 15 | aegis (operator) | `35ab3581f6db` | `/nas/Temp/repos/aegis` @`370e5679d489` | no | candidate; verify ancestry |
| 16 | Control-Center (operator) | `7d432d2251ee` | `/nas/Temp/repos/Control-Center` @`07db7d0c2aeb` | no | candidate; verify ancestry |
| 3 | ci-loop-test | `e170ec6b2b38` | none | no | source unknown (U4); may be disposable-by-name — decision required, not assumed |
| 12 | control-center-api-canary-20260831 | `cf645ff70076` | none | no | source unknown (U4) |
| 1 | codex-trigger-fixture | `0e4f640b700b` | none | no | out-of-root legacy row (I2); source unknown |
| 2 | disposable-canary-20260830 | `dc1f95b47590` | none | no | source unknown (U4) |
| 6,9 | aegis-platform-validation r1/r4 | `5a5c7429…` / `cc3394ad…` | none | no | validation fixtures; r2/r3 never ran; source unknown |
| 7,8 | aegis-platform-validation r2/r3 | — | none | no | zero runs — existence never provable from DB (I3) |
| 18 | Harness | — | none | no | zero runs; source unknown (U4) |

No candidate above is a recovery authority: each needs the
`merge-base --is-ancestor <last-db-hash>` gate (U5) before it can be
declared a superset, and every "no NAS candidate" row stays **unknown**, not
"not found", until its bounded search completes (never label an
inaccessible source as absent — TaskWizer handoff, completion criterion 7).

## 8. Fleet recovery phases

Nothing in this section has run. Phase numbering extends the TaskWizer
handoff's Phase 0–3 shape to N repos.

- **Phase 0 — Preserve (read-only; first action anyone takes).**
  Take the standing DB snapshot (TaskWizer Phase 0 step 2 — sqlite backup
  API against `mode=ro`, into `/home/mkinney/backups/gitforge/`, sha256
  receipt). Archive §3 census output and per-row run receipts under
  task-owned `/nas/Temp/work/` scratch. Execute the remaining bounded
  discovery checks of §7. Run the root-gated snapshot hunt (U2) or record
  it as explicitly unavailable with follow-up.
- **Phase 1 — Decide per row (evidence-gated, no mutations).** For each of
  the 15 missing rows record one disposition with evidence links:
  *restore* (verified superset source exists per §7 gates), *retain as
  documented husk* (row stays, storage intentionally absent, P2 state when
  shipped), or *retire with receipt* (owner-confirmed dead; requires an
  identified owner — U4 blocks most retirements today). Zero-run rows
  (7, 8, 18) default to *retain*, never *retire*, until an owner speaks.
- **Phase 2 — Execute restores (per-repo, TaskWizer template).** Each
  restore follows the TaskWizer handoff's Phase 1–2 exactly (dot-prefixed
  private staging sibling under `runtime-repos`, `umask 077`, mirror copy —
  never a re-clone of a mirror — staging gates incl. `fsck --full
  --strict`, then same-parent `mv -T` publish). One repo at a time, lowest
  blast-radius first (suggest 14 → 15 → 16 → 10 → 11, then 13 via its own
  handoff); the six live rows are never touched.
- **Phase 3 — Prove per restored row (§10 proof, receipts archived).**
  No row advances to "restored" without clone/fetch + real-commit pipeline
  evidence.
- **Phase 4 — Close.** Re-run the §3 census: every missing path must map to
  a recorded disposition; ledger entries appended (F-series discipline,
  docs/planning/MASTER_PLAN_2026-09-20.md — append-with-verdict only);
  prevention controls P1–P5 dispositioned.

## 9. Same-filesystem atomic publish semantics

Verified facts (2026-10-11): `/home` is a single btrfs subvolume mount
(`btrfs /dev/nvme1n1p3[/home]`, O5); `runtime-repos` and its parent
`gitforge-audit-20260822` share device `49` (`stat -c '%d'`, O5).
Consequently `rename(2)` — `mv -T` from a staging sibling directly inside
`runtime-repos` to the absent registered path `<repo-id>` — flips the path
from absent to complete atomically; no observer can see a half-published
repo. Gates before every publish (fail-closed, from the TaskWizer handoff
A2, re-verified per repo at execution time):

```bash
findmnt -T "$STAGE" -o TARGET,SOURCE,FSTYPE
findmnt -T "$LIVE_PARENT" -o TARGET,SOURCE,FSTYPE   # same SOURCE/FSTYPE
stat -c '%d %n' "$STAGE" "$LIVE_PARENT"             # same device id
test ! -e "$LIVE" || { echo 'live path appeared; stop'; exit 1; }
```

Staging must be a **normal directory created directly inside
`runtime-repos`** (not a nested subvolume, not a bind mount), named so it
cannot collide with any `<repo-id>` (dot-prefix convention). Because the
engine resolves paths only as `GIT_ROOT/<repo-id>`
(crates/gitforge-core/src/storage.rs:85-87, §5), a staging sibling is
invisible to serving and publication needs no service restart; restart only
if post-publish diagnostics show stale handles (TaskWizer handoff A3).

## 10. Per-repo proof: clone/fetch and a real-commit pipeline run

Durable-evidence law: transport impressions are diagnostics; the DB is the
receipt. For each restored row, in order:

1. Refs over SSH: `git ls-remote ssh://gitforge@192.168.1.202:42022/<owner>/<repo>.git`
   returns the expected `refs/heads/main` (or the row's primary branch) at
   the recovery-source tip SHA.
2. Smart HTTP + real clone: authenticated `git ls-remote http://192.168.1.202:42782/<owner>/<repo>.git`,
   then `git clone` into task-owned scratch and `rev-parse HEAD` matching
   the source receipt. Credentials only via the configured helper — never
   in URLs or argv. Classify 401/403/404 with server evidence before
   judging; an unauthenticated 200 is not a successful clone (TaskWizer
   handoff Phase 3 rules).
3. Real-commit pipeline: push the verified existing commit to a throwaway
   branch (no synthetic commits, `main` untouched, never force-push), then
   read the run back read-only from the DB. Gate: a new `pipeline_runs` row
   with non-zero `commit_hash` equal to the pushed SHA and a job that
   executed with a populated workspace (contrast PA F7's empty
   `/workspace`). A `failed` run **with a real hash** is storage-success +
   build-failure — record as such; only an all-zero hash means the restore
   did not take.
4. Row-unchanged check: the `repositories` row byte-identical to its
   Phase-0 receipt (id, owner, `git_path` preserved — recovery never edits
   the DB).

## 11. Rollback

Recovery never changes an existing database row: the `repositories` row
must remain byte-identical (§10.4). Verification pushes append
`pipeline_runs` evidence (§10.3); no existing run or row is edited/deleted.

- Before §10 passes: `mv -T "$LIVE" "$STAGE"` — move the published tree
  back to the staging name and **preserve it there**; never delete or
  overwrite recovery data. The system returns to the pre-publish routing
  state (registered path absent, row untouched).
- Throwaway verification branches: delete over SSH transport only
  (`git push origin --delete <branch>`), never force-push.
- `pipeline_runs` is append-only evidence — never delete rows by hand;
  record unwanted runs as recovery receipts.
- Any eventual tree deletion follows host discipline (enumerated paths,
  manifest first, `find <path> -depth -delete`); `rm -rf` wildcards are
  forbidden on this host.

## 12. Owners

- **PA operator / Fedora host owner (mkinney)**: executes Phases 0–3
  gates; owns the privileged U2 snapshot check (record "unknown", never
  "absent", if sudo is unavailable); owns any engine-restart fallback.
- **GitForge maintainer**: owns P1–P5 (§6), the `delete_repo` asymmetry
  ADR, and the CLI 401 fix already delegated in the TaskWizer handoff
  (Phase 3 alternative trigger).
- **Per-repo owners**: TaskWizer → GitHub `aliasfoxkde` (per TaskWizer
  handoff). All other rows: **owner unknown (U4)** — identification is a
  Phase-1 input; do not invent or assume ownership. No retirement may
  proceed on an unowned row.

Dependencies: GitHub repos (O9 origins) stay reachable until each Phase 2
completes; NAS candidate clones (O8) must not be pruned until Phase 3
passes for their row; the NAS `taskwizer-rust-cli` clone is additionally
the PA S2 fallback and `6e005c62` witness.

## 13. Task list

1. [ ] Fleet Phase 0: standing DB snapshot + sha256 receipt into
   `/home/mkinney/backups/gitforge/` (PA Phase 0 step 2 verbatim).
2. [ ] Fleet Phase 0: archive §3 census + per-row run receipts to
   `/nas/Temp/work/gitforge-fleet-integrity-20261011/`.
3. [ ] Fleet Phase 0: complete bounded discovery — per-row GitHub
   ancestry gates (`merge-base --is-ancestor <last-db-hash>`) for rows
   10, 11, 14, 15, 16; record U4 owner identification attempts.
4. [ ] Fleet Phase 0: U2 root-gated snapshot hunt executed or recorded
   unavailable-with-follow-up (fleet-wide S0 sweep).
5. [ ] Phase 1: disposition record (restore / husk / retire) for each of
   the 15 rows, each citing §7 evidence; owner sign-off where required.
6. [ ] Phase 2: execute restores in ascending blast-radius order via the
   TaskWizer template (§8); one row at a time; receipts per row.
7. [ ] Phase 3: §10 proof per restored row; receipts appended.
8. [ ] TaskWizer row: execute and close per its own handoff (§2) — tracked
   there, not here.
9. [ ] Prevention: P1 reconcile metric/alert shipped; P2 ADR written;
   P3–P5 dispositioned (§6).
10. [ ] Phase 4: closing census re-run; ledger entry appended citing this
    handoff and all receipts.

## 14. Exit criteria (measurable)

1. All 15 missing rows have an evidence-backed disposition:
   restored-and-proven, husk-retained (documented), or retired-with-receipt.
   The 6 surviving rows are receipted as live in the closing census (§8
   Phase 4). Zero missing rows remain "unknown" except explicitly
   unavailable sources recorded with follow-ups (never labeled absent).
2. Every *restored* row passes §10: both transports answer `ls-remote`
   with the receipt SHA; a fresh clone's `rev-parse HEAD` matches; a new
   non-zero-`commit_hash` run exists with a populated job workspace; the
   `repositories` row is byte-identical to its Phase-0 receipt.
3. A re-run of the §3 census reports 0 **unexplained** absences (absent
   paths exist only where the disposition is husk/retire with a recorded
   decision).
4. Prevention shipped: P1 metric alerting live (and catches a seeded
   missing-dir test), P2 ADR merged, backup cadence documented in
   docs/RUNBOOK.md.
5. TaskWizer handoff completion criteria (its §Completion criteria) are
   met for `f18e9d03` — fleet closure does not preempt or substitute for
   them.
6. Ledger discipline: every disposition row cites file/row/command
   evidence; no invented owners, causes, or pipeline behavior anywhere in
   the closure record.

## 15. Non-mutations statement (authoritative as of 2026-10-11)

No restore, republish, staging directory, push, clone into live storage,
service restart, or Fedora DB/path/service mutation has occurred under this
handoff or its investigation. No missing repository has been recreated or
deleted. Credentials remain environment-only and were never read or
printed. The first action anyone may take is Fleet Phase 0 (§8).
