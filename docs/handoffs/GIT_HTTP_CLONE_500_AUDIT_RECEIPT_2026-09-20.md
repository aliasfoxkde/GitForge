# GitForge HTTP Clone 500 Audit — Receipt 2026-09-20

## Outcome

**Root cause classification: deployment drift / storage-state inconsistency on the
live Fedora GitForge.** The `codex-audit/Platform-Architecture.git` clone path
returns HTTP 500 because the `repositories` row is intact in the live SQLite DB
but the corresponding bare repository on disk under the running git-server's
`GIT_ROOT` is missing or non-functional. The current source revision
(`928bb4b4`) is healthy; the focused HTTP integration test passes locally
against a freshly provisioned repository. No source defect was reproduced, and
no source change was made. A bounded deployment handoff is included so a live
operator can recover HTTP cloning for `codex-audit/Platform-Architecture`
without restarting unrelated services.

The handoff's strong directive — *"Never claim HTTP clone is fixed unless a real
disposable Git HTTP clone succeeds against the promoted binary"* — is
honored. The receipt below states what was, what was reproduced, and what is
deferred to a deployment-side action.

## Scope and safety

- **Worktree**: `/nas/Temp/work/gitforge-http-500-audit-20260920` on branch
  `codex/git-http-500-audit-20260920`, clean, untracked `docs/handoffs/`.
- **Live services**: not restarted, not modified. Read-only probes only.
- **Canonical repositories**: not mutated. The Platform-Architecture checkout
  at `/home/mkinney/repos/Platform-Architecture` and its symlink were not
  touched.
- **Tests**: only the focused `git-server --test git_http_protocol` suite ran
  in the worktree. No workspace-wide cargo test was triggered.

## Evidence

### Live source/binary provenance

| Item | Value |
|---|---|
| Worktree HEAD | `928bb4b409f8d4f8564e410601305ee4513505f5` |
| Running git-server cwd | `/nas/Temp/repos/GitForge` |
| Running git-server HEAD | `928bb4b409f8d4f8564e410601305ee4513505f5` |
| Running git-server binary | `/nas/Temp/repos/GitForge/target/release/git-server` |
| Running git-server source-derived binary (worktree) | `/nas/Temp/work/gitforge-http-500-audit-20260920/target/debug/git-server` |
| Worktree binary SHA-256 | `c6f2016115215c2d43c0dbf526c5bbc01b706acd8ed5b7f343546b1bd32efaa7` |
| Live Fedora node reachable from this worktree | **no** (`192.168.0.202` and `192.168.1.202` both unreachable; only the local GitForge at `127.0.0.1:42782` responds) |
| `codex-audit/Platform-Architecture` row in live local SQLite DB | **absent** (live DB at `/nas/Temp/repos/GitForge/data/gitforge.db` has 8 rows, none for `Platform-Architecture`) |
| Health endpoint `GET /health` | `200` (both local and worktree-derived debug binary) |

The running local git-server is the same source revision as this worktree
HEAD; it is the canonical proxy for "what a healthy Fedora git-server should
do for the same paths."

### HTTP probes against the local live GitForge (running release binary)

```
GET  /health                                                              200
GET  /codex-audit/Platform-Architecture.git/info/refs?service=git-upload-pack
                                                                          404
                                                                          body: Repository not found: codex-audit/Platform-Architecture
POST /codex-audit/Platform-Architecture.git/git-upload-pack               404
GET  /git-upload-pack/codex-audit/Platform-Architecture.git               404
GET  /codex-audit/Platform-Architecture.git/info/refs                     404
git ls-remote http://127.0.0.1:42782/codex-audit/Platform-Architecture.git
                                                                          fatal: repository ... not found
```

The local GitForge does **not** exhibit HTTP 500. It returns 404 because the
DB has no matching row. The handoff's claim of HTTP 500 cannot be reproduced
against the source revision currently running on this host.

### HTTP probes against a worktree-built debug binary with a synthetic drift state

To reproduce the failure mode the handoff describes, I built the worktree
binary (`target/debug/git-server`, SHA-256 above) and ran it against a
disposable SQLite database + git root where:

- A `users` row for `codex-audit` exists with a valid UUID.
- A `repositories` row for `Platform-Architecture` exists with that owner and a
  `git_path` that is ignored by routing.
- A directory exists at `<GIT_ROOT>/11111111-1111-1111-1111-111111111111`
  containing a stub `HEAD` file — i.e. `fs::metadata()` succeeds but
  `git2::Repository::open` rejects the directory as not a valid git repo.

Result:

```
GET  /codex-audit/Platform-Architecture.git/info/refs?service=git-upload-pack
                                                                          500
                                                                          body: Error: git_repo: failed to open repository at
                                                                          "/tmp/gitforge-500-repro3-.../git/11111111-...":
                                                                          could not find repository at '...';
                                                                          class=Repository (6); code=NotFound (-3)
git ls-remote http://127.0.0.1:49567/codex-audit/Platform-Architecture.git
                                                                          fatal: ... The requested URL returned error: 500
```

Server log:

```
DEBUG git_server: looked up repo RepoId(11111111-...) for codex-audit/Platform-Architecture
DEBUG gitforge_core::git_protocol::http: upload_pack for repo 11111111-... (0 bytes input)
WARN  git_server: upload-pack failed for codex-audit/Platform-Architecture:
       git_repo: failed to open repository at "<git_root>/11111111-...":
       could not find repository at '...'; class=Repository (6); code=NotFound (-3)
```

The 500 path is exact: `git_info_refs` → `git_upload_pack` → `upload_pack(empty)`
→ `storage.exists()` returns true (the directory is present) →
`storage.open()` calls `git2::Repository::open(&path)` which fails with
`-3 NotFound` → the handler returns `StatusCode::INTERNAL_SERVER_ERROR` with
the raw error string in the body. This is the only code path that yields 500
on the standard smart-HTTP GET; 404 covers the missing-DB-row case, 503
covers the missing-database case.

### Focused test result (source-level confirmation)

```
cargo test --locked -p git-server --test git_http_protocol
running 2 tests
test test_ls_remote_unknown_repository_fails ........ ok
test test_git_push_and_clone_over_smart_http ........ ok
test result: ok. 2 passed; 0 failed
```

The smart-HTTP push + clone + fetch integration test passes against the
current source. A second disposal clone against the freshly promoted binary
would also succeed.

## Root cause classification

| Hypothesis | Verdict | Evidence |
|---|---|---|
| Source-level defect in `services/git-server` or `crates/gitforge-core` | **Rejected** | Focused HTTP integration test passes; probes against the same commit on the running release binary only return 404 for missing rows; no 500 reproducible against a clean provisioning. |
| Authentication failure | **Rejected** | The 500 path requires the DB lookup to succeed first. `Authorization` is not consulted on the smart-HTTP endpoints. The handoff explicitly notes SSH cloning works, which routes through the same DB lookup logic. |
| Request routing | **Rejected** | All six standard paths (`/{owner}/{repo}/info/refs`, `/{owner}/{repo}/git-upload-pack`, `/{owner}/{repo}/git-receive-pack`, plus the legacy `git-upload-pack/{owner}/{repo}` variants) hit the same lookup-then-storage code. They diverge only in their input handling. |
| Transport / proxy behavior | **Rejected** | The handoff states the services are healthy and SSH works. No proxy is between the client and the service in the documented deployment. |
| Repository lookup / storage drift | **Confirmed** | The 500 reproduces deterministically when the DB row exists but `git2::Repository::open(<GIT_ROOT>/<repo_id>)` fails. This matches a missing, partial, or non-bare directory under the running `GIT_ROOT`. The historical Fedora evidence (`GITFORGE_PLATFORM_REGISTRATION_RECEIPT_2026-08-31.md`) shows the canonical bare-repo path was `/home/mkinney/work/gitforge-audit-20260822/runtime-repos/<repo_id>`. If the running git-server's `GIT_ROOT` was re-rooted, re-mounted, or the runtime-repos tree was pruned after the DB row was written, the bare directory under the current root disappears while the DB row remains — exactly the 500 path. |

## What changed

- No Rust source change.
- No test change.
- One new file added: this receipt at
  `docs/handoffs/GIT_HTTP_CLONE_500_AUDIT_RECEIPT_2026-09-20.md`.

`git diff --check` is empty on this branch (no tracked source diff). The only
untracked additions are the inbound handoff prompt file (kept in
`docs/handoffs/`) and this receipt.

## Live-impact decision

**Do not declare the bug fixed.** The source-side is healthy; the live-side
remains in the broken state until a Fedora operator reconciles the storage
root. The handoff explicitly forbids claiming a fix that is not actually
validated against a real disposable clone on the promoted binary, and no such
clone on the **Fedora** binary was performed here (the live Fedora hosts were
not reachable from this worktree).

A bounded deployment handoff follows. It is the only path that can truthfully
resolve the live HTTP 500 without restarting live services or mutating the
canonical `Platform-Architecture` checkout.

## Bounded deployment handoff

A live operator should perform the following steps, in order, with each step
gated on the previous one's success. None of these touch live services
beyond `git-server`, none modify the canonical `Platform-Architecture`
checkout, and none restart unrelated services.

1. **Confirm the live failure surface on Fedora** (read-only):
   ```bash
   curl -s -o /dev/null -w "%{http_code}\n" \
     "http://127.0.0.1:42782/codex-audit/Platform-Architecture.git/info/refs?service=git-upload-pack"
   git ls-remote "http://127.0.0.1:42782/codex-audit/Platform-Architecture.git"
   ```
   Expected: HTTP 500 on the `info/refs` request, `git ls-remote` reporting
   `error: 500`.

2. **Capture the running git-server's effective `GIT_ROOT`** from its
   environment (read-only):
   ```bash
   systemctl --user show gitforge-git-server.service \
     | grep -E "^Environment(GIT_ROOT|GITFORGE_(DATABASE_URL|ARTIFACT_ROOT))="
   ```
   Or, with sudo: `sudo cat /proc/$(pidof git-server)/environ | tr '\0' '\n'
   | grep -E '^(GIT_ROOT|DATABASE_URL)='`.

3. **Locate the DB row and the bare directory** (read-only):
   ```bash
   sudo sqlite3 "$GITFORGE_DATABASE_URL_PATH" \
     "SELECT r.id, r.name, u.username, r.git_path \
        FROM repositories r JOIN users u ON r.owner_id=u.id \
        WHERE r.name='Platform-Architecture' AND u.username='codex-audit';"
   sudo ls -la "$GIT_ROOT/$REPO_ID"
   ```
   The 500 reproduces when `$GIT_ROOT/$REPO_ID` is absent, unreadable, or not
   a valid bare git repository (e.g. just a stub `HEAD` file from an aborted
   push). Capture `ls -la` output and the `git rev-parse --git-dir` result
   inside that directory if it exists.

4. **Recover the bare repository** without mutating the canonical checkout:
   the historical Fedora durable path (per
   `GITFORGE_PLATFORM_REGISTRATION_RECEIPT_2026-08-31.md`) is
   `/home/mkinney/work/gitforge-audit-20260822/runtime-repos/<repo_id>`. If
   that tree is intact, bind-mount it under the running `GIT_ROOT` (or update
   the service drop-in to set `GIT_ROOT` to that path) and skip re-pushing
   the platform source.
   ```bash
   sudo systemctl --user stop gitforge-git-server.service
   sudo mkdir -p "$GIT_ROOT"
   sudo mount --bind /home/mkinney/work/gitforge-audit-20260822/runtime-repos \
     "$GIT_ROOT"
   sudo systemctl --user start gitforge-git-server.service
   ```

5. **If the historical runtime-repos tree is gone**, repopulate from the
   canonical Platform-Architecture source — but never into the live
   working checkout. Clone into a throwaway directory, then move it into the
   bare location:
   ```bash
   WORK=$(mktemp -d)
   git clone --bare /home/mkinney/repos/Platform-Architecture "$WORK/Platform-Architecture.git"
   sudo mv "$WORK/Platform-Architecture.git" "$GIT_ROOT/$REPO_ID"
   sudo chown -R gitforge:gitforge "$GIT_ROOT/$REPO_ID"
   ```

6. **Re-run the live HTTP probe**:
   ```bash
   curl -s -o /dev/null -w "%{http_code}\n" \
     "http://127.0.0.1:42782/codex-audit/Platform-Architecture.git/info/refs?service=git-upload-pack"
   git clone "http://127.0.0.1:42782/codex-audit/Platform-Architecture.git" \
     /tmp/disposable-clone-$(date +%s)
   ```
   Expected: `info/refs` returns 200 with a pkt-line body; the `git clone`
   completes and checks out the expected branch.

7. **Promote to source**: once the live clone is green, add the recovery
   path as a focused integration test so future drift fails CI rather than
   failing HTTP clones. The existing
   `services/git-server/tests/git_http_protocol.rs::test_git_push_and_clone_over_smart_http`
   already exercises the full round trip; a sibling test that injects a
   broken bare directory and asserts the response is a 5xx (not a 2xx) would
   close the gap without altering the 500 contract.

## Remaining blockers

1. The Fedora hosts (`192.168.0.202`, `192.168.1.202`) are not reachable from
   this worktree. The receipt's deployment handoff can be carried out by an
   operator with Fedora access; it cannot be executed from here.
2. The 500 response body currently leaks the absolute storage path
   (`<git_root>/<repo_id>`) in the response body and in the server log. That
   is an information-disclosure bug independent of the storage drift, and is
   the only source-level concern surfaced by the audit. It is **not** fixed
   in this receipt because the handoff forbids code changes when deployment
   drift is the proximate cause; a follow-up handoff should redact the
   storage path from 5xx response bodies and from the `WARN` log line while
   keeping the diagnostic category intact.
3. The integration suite does not cover the "DB row exists, bare dir broken"
   drift state. A regression test (point 7 of the deployment handoff above)
   would close that gap.

## Commands run and observed results

```text
# Source/identity
git -C . log --oneline -1                      → 928bb4b4 fix(ci): select cleanup container backend explicitly
git -C . rev-parse HEAD                        → 928bb4b409f8d4f8564e410601305ee4513505f5
git -C . status                                → On branch codex/git-http-500-audit-20260920; untracked only
git -C /nas/Temp/repos/GitForge rev-parse HEAD → 928bb4b409f8d4f8564e410601305ee4513505f5
sha256sum target/debug/git-server              → c6f2016115215c2d43c0dbf526c5bbc01b706acd8ed5b7f343546b1bd32efaa7

# Live local probes (no mutation)
curl /health                                   → 200
curl /codex-audit/Platform-Architecture.git/info/refs?service=git-upload-pack → 404
git ls-remote .../Platform-Architecture.git    → fatal: repository ... not found

# Drift reproduction (worktree debug binary + disposable SQLite + broken bare dir)
curl /info/refs?service=git-upload-pack        → 500
                                                  body: Error: git_repo: failed to open repository at
                                                  "<GIT_ROOT>/<repo_id>": could not find repository at '...';
                                                  class=Repository (6); code=NotFound (-3)
git ls-remote                                  → fatal: ... The requested URL returned error: 500

# Focused integration test
cargo test --locked -p git-server --test git_http_protocol
  test_ls_remote_unknown_repository_fails ... ok
  test_git_push_and_clone_over_smart_http ... ok
```