# PR #191 Review Follow-ups

Branch: `codex/pr191-followups-20260922` (isolated worktree,
`/nas/Temp/work/gitforge-pr191-followups-20260922`), based on candidate
`22f4c58e8ec910d69e66a0db3a057a0f355d93c2`.

This document records how each independent-review follow-up was implemented,
and why the one item that could not be implemented faithfully was documented
instead of faked.

## Follow-up 1 — genuine mid-`drive` cancellation test

**Status: implemented.**

New test `test_git_rpc_child_drive_cancelled_mid_await_reaps_live_child`
(`crates/gitforge-core/src/git_protocol/http.rs`). Unlike the existing
abandon-reap tests, which drop the guard *before* `drive` runs, this test
proves cancellation *while `drive` is parked at an await point with a live
child*:

1. Spawn `sh -c 'exec sleep 30'` with fully piped stdio (`exec` guarantees the
   tracked pid is the long-lived process itself) and record its pid.
2. Create `guard.drive(Vec::new())` (empty input makes `write_all` a no-op, so
   the future parks on the stdout/stderr `read_to_end` drain).
3. `Box::pin` the future — the box owns the future and therefore the guard, so
   dropping the box actually runs `GitRpcChild::drop`.
4. Poll exactly once with `futures::task::noop_waker_ref` and assert
   `Poll::Pending`. `sleep 30` produces no output, so the pipes stay open and
   the single poll deterministically parks on the drain. This assertion is the
   proof that the subsequent drop happens mid-`drive`, not after completion;
   if a future change made `drive` complete instantly, the test fails loudly
   instead of silently losing coverage.
5. Drop the future (abort) and assert `waitid(WNOWAIT|WNOHANG)` fails with
   `ECHILD`, i.e. the still-alive child was SIGKILLed and fully reaped — no
   zombie, no reliance on tokio's best-effort orphan queue.

The test is deterministic (no sleeps, no racing child) and bounded (single
poll; the child lives ~milliseconds before Drop kills it; worst case the 30 s
`sleep` would be reaped by the Drop grace logic, never blocking the test).

During development the harness itself was validated: an intermediate version
dropped a `Pin<&mut F>` (from `tokio::pin!`), which does *not* drop the
future, and the test correctly failed with "pid still exists (not reaped)" —
demonstrating the reaped-assertion genuinely detects unreaped children.

## Follow-up 2 — stdout/stderr pipe backpressure regression test

**Status: implemented.**

New test `test_git_rpc_child_drive_drains_stderr_backpressure_concurrently`
(`crates/gitforge-core/src/git_protocol/http.rs`). A bounded stand-in child
(`sh -c 'exec head -c 262144 /dev/zero 1>&2'` — a single process writing
exactly 256 KiB, four times the default 64 KiB Linux pipe capacity, to
stderr, then exiting) is driven through `drive`. `drive` must drain stdout and
stderr concurrently while the child runs; if the drain regressed to reading
the pipes only after the child exits, `head` would block forever on a full
pipe and never exit. The test wraps `drive` in a `tokio::time::timeout` of
30 s and asserts completion, exit status success, and empty stdout — proving
the concurrent drain completes without hanging.

## Follow-up 3 — move `libc` to test-only dependencies

**Status: implemented for `gitforge-core` (the affected crate); correctly left
in place for `gitforge-process`.**

Audit result:

- `crates/gitforge-core`: `libc` was referenced *only* inside the `#[cfg(test)]`
  child-lifecycle module (`http.rs`, `waitid`/`ECHILD` assertions). The
  dependency was moved from `[dependencies]` to `[dev-dependencies]` with a
  comment recording why. Verified with `cargo tree -p gitforge-core -e normal
  --depth 1 | grep -c libc` → `0`, and `-e dev` still shows `libc v0.2.189`.
- `crates/gitforge-process`: `libc` is used by production code
  (`subreaper.rs`: `prctl(PR_SET_CHILD_SUBREAPER)`; `signal.rs`:
  `waitpid(-1, WNOHANG)` reaper) and therefore stays a real dependency.
  Moving it would break the build.

Locked builds remain correct: `cargo build --locked -p gitforge-core -p
gitforge-process` and `cargo test --locked -p gitforge-core --no-run` both
succeed, and `Cargo.lock` needed no changes (libc was already resolved for the
dev graph, so the lockfile is unchanged).

## Follow-up 4 — explicit cfg boundaries for Unix-only lifecycle tests

**Status: implemented.**

The child-lifecycle suite in `crates/gitforge-core/src/git_protocol/http.rs`
relies on POSIX process semantics (`/bin/sh`, `waitid`, SIGKILL semantics) and
previously compiled only by accident on other targets. Changes:

- Every item in the suite is now explicitly gated with `#[cfg(unix)]`:
  helpers `spawn_test_child_script`, `spawn_test_child`,
  `spawn_test_child_holding_open`, `assert_pid_reaped`, and the six
  lifecycle `#[tokio::test]`s (`drive_reports_success_status`,
  `drive_maps_child_error`, `abandoned_mid_flight_is_reaped_not_zombied`,
  `drop_after_reap_is_noop`, `drive_cancelled_mid_await_reaps_live_child`,
  `drive_drains_stderr_backpressure_concurrently`,
  `upload_pack_child_spawn_is_reaped_when_abandoned`).
- `spawn_test_child` was refactored over a new `spawn_test_child_script`
  helper so the two new tests can spawn custom stand-in children without
  duplicating the piped-stdio setup.
- Linux coverage is unchanged: on Linux every gated test still compiles and
  runs (verified below). The pure-logic tests (`parse_*`,
  `test_abandoned_child_grace_is_bounded`, storage/repo tests) remain
  ungated and therefore still run on every platform.
- A comment on the section header now states the gating contract.

## Validation performed

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy -p gitforge-core --all-targets --all-features -- -D warnings` | pass |
| `cargo test -p gitforge-core --lib` | 89 passed, 0 failed |
| `cargo test -p gitforge-core --lib git_protocol::http::tests` | 21 passed (was 19; +2 new) |
| focused lifecycle tests, 5 repeated runs, `--test-threads=1` | 6 passed, 0 failed, every iteration |
| `cargo build --locked -p gitforge-core -p gitforge-process` | pass |
| `cargo test --locked -p gitforge-core --no-run` | pass |
| `git show --check HEAD` | clean (no whitespace errors) |
| `cargo check --workspace --all-targets` | pass |

Toolchain: cargo/rustc 1.98.1.

## What was intentionally not done

- The synchronous `Drop` abandon-reap policy (SIGKILL + bounded 1 s
  synchronous `try_wait` poll, then best-effort hand-off to the runtime
  orphan queue) was **not** redesigned, per task constraints. The new
  cancellation test pins the existing contract rather than changing it.
- No production protocol behavior was modified; the production diff is
  limited to moving the `libc` declaration in `gitforge-core/Cargo.toml`.
- No gate was weakened; two tests were added and existing Linux coverage was
  preserved.

## Remaining limits

- The cancellation test polls once with a no-op waker rather than through a
  full runtime scheduler tick; this is deliberate for determinism (a
  scheduler-driven drop could race the child's exit). It still proves the
  drop happens inside `drive` (Pending assertion) at a point where the child
  is alive, which is the property the reviewer asked to pin.
- `sh`, `head`, and `/dev/zero` are assumed present on Unix CI images; the
  suite already assumed `/bin/sh` before this change. On non-Unix targets the
  new tests are compiled out (follow-up 4), which is the correct trade-off —
  the guarded production code paths remain exercised on Linux.
- Non-Unix behavior of the lifecycle code is untested by design; there is no
  Windows CI target configured in this repository to validate one.
