# PR #293 Receipt — Runner PATH fix for Security / Rust CI jobs (2026-10-10)

- **PR**: aliasfoxkde/GitForge#293 (`codex/codeql-cargo-path-fix-20261010`)
- **Base**: `main` @ `c4e3816bf19dd3a4ca06b9861257e4a925ad3c71`
- **Head at diagnosis**: `6ce8b801c294be1174c679ce9f97aff0e8479ab7`
- **Fix commit**: `abbb9dd91f508f8709951ef9902842a0cab9d553`
  (`fix(ci): expose runner rustup to CI jobs`) — the commit directly
  below this receipt commit on the PR branch.
- **Scope of fix**: `.github/workflows/security.yml`, `.github/workflows/rust-ci.yml`
  only. No gate semantics changed, no suppressions added, no `.rs` sources touched.

## 1. Confirmed failures at the diagnosis head (`6ce8b801`)

Source: `GET /repos/aliasfoxkde/GitForge/actions/runs/38100998153/jobs` (Security
workflow run, PR head), step-level status/conclusions.

| Check run id | Job | Failing step | Duration (UTC) | Runner |
|---|---|---|---|---|
| 114356725358 | Cargo Audit | step 3 "Install Rust" (dtolnay/rust-toolchain) | 01:11:59Z, 0s | fedora-gitforge-01 (id 21) |
| 114356725397 | Rust Security | step 3 "Install Rust" | 01:13:08Z, 0s | fedora-gitforge-01 (id 21) |
| 114356725382 | Aegis Changed-Line Scan | step 3 "Build pinned Aegis scanner" (`cargo build`) | 01:23:05→01:23:15Z | fedora-gitforge-01 (id 21) |

All downstream steps in those jobs were `skipped`. `secrets-scan`
(114356725139) succeeded on the same runner in the same run — it never invokes
cargo/rustup, isolating the variable to the Rust toolchain provisioning.

## 2. Old-run vs current-SHA classification

Source: `GET /actions/workflows/security.yml/runs?branch=main` and
`GET /actions/runs/38074059170/jobs` (main @ `c4e3816b` = this PR's merge base,
push event 2026-10-10 18:00–18:06Z, ~7h before the PR runs).

| Check run id | Job | Failing step | Duration | Runner |
|---|---|---|---|---|
| 114277219442 | Cargo Audit | step 3 "Install Rust" | 0s | fedora-gitforge-01 (id 21) |
| 114277219574 | Rust Security | step 3 "Install Rust" | 0s | fedora-gitforge-01 (id 21) |
| 114277219623 | Aegis Changed-Line Scan | step 3 "Build pinned Aegis scanner" | 1s | fedora-gitforge-01 (id 21) |

Main-branch history for the same workflow: **success** @ `19daeea9`
(runs 37312337172 on 2026-10-05, 37190602635 on 2026-10-04) → **failure**
@ `263b2b95` (run 37545705327, 2026-10-06) → **failure** @ `54f93c3b`
(run 37771873726, 2026-10-08) → **failure** @ `c4e3816b` (run 38074059170,
2026-10-10). The regression window is Oct 5–6 on the runner environment and
**predates this PR**. Identical step signatures at base and head rule out the
PR diff as the cause; all failing steps execute before any repo-content-
sensitive work (checkout succeeds, toolchain provisioning fails).

## 3. Root cause

The Fedora self-hosted runner (`fedora-gitforge-01`) has rustup under
`$HOME/.cargo`, but job `PATH` omits `$CARGO_HOME/bin`:

1. Jobs invoking `dtolnay/rust-toolchain` ("Install Rust") fail instantly:
   the action finds no `rustup` on PATH and falls through to its rustup
   bootstrap, which fails on this host.
2. Jobs invoking bare `cargo` fail in ~1–2 s (exit 127-class): the runner
   ships no `cargo` on PATH.

This is the same defect class already diagnosed and fixed for CodeQL in this
PR (`codeql.yml` "Expose runner rustup on PATH", citing runs 38093795938 and
38094440919, including the /tmp quota constraint on this host).

## 4. Fix (this commit)

Port the established `codeql.yml` pattern, nothing else:

- **security.yml** — "Expose runner rustup on PATH" added to `cargo-audit`,
  `rust-security`, and `aegis-scan`; `aegis-scan` additionally gets the
  pinned `dtolnay/rust-toolchain` stable install (it previously relied on a
  pre-existing `cargo` that no longer exists) and `TMPDIR: ${{ runner.temp }}`
  on the scanner build; `rust-security` clippy step gets `TMPDIR`.
- **rust-ci.yml** — "Expose runner rustup on PATH" added to the seven
  self-hosted jobs (`fmt`, `clippy`, `test`, `test-serialized`, `coverage`,
  `security`, `build`); `TMPDIR: ${{ runner.temp }}` added to the five jobs
  that build the workspace (`clippy`, `test`, `test-serialized`, `coverage`,
  `build`), mirroring the documented aws-lc-sys TMPDIR failure on this host.
- Untouched by design: `secrets-scan` (no cargo), `shellcheck`,
  `workflow-lint`, `supply-chain` (run on GitHub-hosted `ubuntu-latest`),
  `integration.yml` (Go-only via setup-go), `codeql.yml` (already fixed in
  this PR), all application code.

Both edited workflows list themselves in their `pull_request` paths filters,
so the push re-triggers them; `cancel-in-progress` concurrency supersedes the
doomed queued runs at `6ce8b801` instead of burning the backlogged runner
(security.yml run 38100998153, rust-ci.yml run 38100998239, integration.yml
run 38100998139, gitforge-ci.yml run 38100998150, try-merge run 38100998212,
CodeQL run 38100998199 at the old head).

## 5. Bounded validation performed (closeout session, 2026-10-10 ~21:05 CDT)

All commands run in the NVMe worktree
(`/home/mkinney/worktrees/gitforge-async-trigger-response-20261010`;
`findmnt -T <worktree>` → `/dev/nvme0n1p2 ext4`, not NAS):

- `git diff --check` — clean (no whitespace errors).
- Conflict-marker grep (`^(<<<<<<<|=======$|>>>>>>>)`) on both files — clean.
- Structural YAML validation via `python3` + PyYAML 6.0.2 (`yaml.safe_load`,
  then walk `jobs`/`steps`/`env`): rust-ci.yml 10 jobs, security.yml 5 jobs;
  every step has exactly one of `uses`/`run`; all job-level `env` mappings
  are dicts (no parse or shape errors). 150 insertions, 0 deletions.
- `shellcheck -s bash -S warning` over all 31 embedded `run:` blocks of both
  workflows — 0 findings (11 of the blocks are this fix's insertions).
- `actionlint` not installed on this host; the authoritative workflow lint is
  the `workflow-lint` job in rust-ci.yml (actionlint v1.7.12, GitHub-hosted
  `ubuntu-latest`, unaffected by this defect) and runs on the pushed head.
- Receipt-claim cross-checks against the parsed YAML: all 7 rust-ci.yml
  self-hosted jobs carry the PATH step and the 5 workspace-building jobs
  carry `TMPDIR`; the 3 cargo-invoking security.yml jobs carry the PATH step
  and TMPDIR where builds run; both workflows list themselves in their
  `pull_request.paths` filters; both set `concurrency.cancel-in-progress:
  true` grouped on `github.workflow`-`github.ref`.
- A stale 0-byte `index.lock` for this worktree (34 min old, no holder per
  `fuser`) was removed before committing; no other git process was affected.
- No local cargo builds/tests run and no GitForge CI queue triggers, per
  tasking constraints (single healthy runner, measured backlog).

## 6. Check status — NOT claimed green

At `6ce8b801` the following were queued/in-progress and were **not** verified
passing; the pushed fix commit replaces them with fresh runs whose outcomes
are pending at receipt time:

- rust-ci.yml (run 38100998239): Format 114356725640, Clippy 114356725712,
  Test 114356725501, Test (Serialized) 114356725709, Coverage 114356725689,
  Build 114356725615, Security Audit 114356725735
- CodeQL (run 38100998199): Analyze (rust) 114356725403 (in progress at old
  head); Analyze (go) 114356725241 passed
- gitforge-ci.yml (run 38100998150): Poll GitForge Build Status 114356749869
- integration.yml (run 38100998139): integration 114356725363, e2e 114356725227
- try-merge (run 38100998212): try-merge 114356725561

Passing at `6ce8b801` (environment-independent or unaffected jobs): Supply
Chain, Lint Workflows, ShellCheck, secrets-scan 114356725139, Enqueue Build to
GitForge 114356725191, check 114356725218. CodeQL 114356934242 was `neutral`
("1 configuration not found", informational); dependency-review 114356725920
skipped by its `vars` gate.

## 7. Constraints honored

No merge, no deploy, no live GitForge service/database/mirror contact, no
credentials printed, no full local builds/tests, no additional queue triggers
beyond the single fix push.
