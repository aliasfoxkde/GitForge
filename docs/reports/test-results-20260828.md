# GitForge P0 Command Wiring Retry — Test Results

**Date:** 2026-08-28
**Branch:** `feature/p0-runner-command-wiring-retry-20260828`

## Summary

All tests pass. The implementation correctly wires job metadata (image + steps) from
the API layer through the scheduler to the runner, and now fails-closed on malformed
or missing image/command metadata before sandbox execution.

## What Was Fixed

### Corrective Pass: Fail-Closed on Missing/Malformed Metadata

An independent audit identified that `execute_job` in `gitforge-runner` had two
divergent behaviors for malformed metadata:

| Field | Before | After |
|-------|--------|-------|
| `image` is empty | **Fail-closed** — reject, report failure to scheduler | unchanged |
| `commands` is empty | **Warn and proceed** — no-op execution | **Fail-closed** — reject, report failure to scheduler |

Both defects allowed malformed execution requests to reach the sandbox. The fix makes
`commands` match the existing `image` enforcement: the runner refuses to execute any
job that lacks validated command metadata, with a structured failure receipt sent to
the scheduler so the job is not left in limbo.

`#[serde(default)]` on `image` (String → `""`) and `commands` (Vec → `[]`) remains
correct: it handles the **missing-field** case during deserialization. The
fail-closed check in `execute_job` then catches the resulting empty value before the
executor runs.

### Fail-Closed Enforcement Chain

```
API layer (trigger_pipeline)
  └─ validates name, image, steps non-empty before persisting

Scheduler (get_pending_jobs)
  └─ skips jobs where image is "" or commands is empty[]

Runner (execute_job)
  └─ if image.is_empty()  → reject, POST failure, return
  └─ if commands.is_empty() → reject, POST failure, return
  └─ build ExecutableJob and call executor.execute()
```

## Test Results

### Clippy
```
cargo clippy --workspace --all-targets -- -D warnings
```
**Result:** PASS — zero warnings or errors.

### Targeted Tests
```
cargo test --package gitforge-runner
```
**Result:** ALL PASS — 64 tests, 0 failed.

### Full Workspace Tests
```
cargo test --workspace
```
**Result:** ALL PASS — all packages, all test suites.

### Formatter
```
cargo fmt --all
```
**Result:** PASS — no diffs.

## Fail-Closed Test Cases (New)

| Test | Input | Expected Behavior |
|------|-------|-------------------|
| `test_job_assignment_empty_image_deserializes_from_missing_field` | JSON missing `image` | Deserializes to `""`; `execute_job` rejects |
| `test_job_assignment_deserialize_with_null_image` | `{"image":null,...}` | Deserialization **fails** (type error) |
| `test_job_assignment_missing_commands_defaults_to_empty_vec` | JSON missing `commands` | Deserializes to `[]`; `execute_job` rejects |
| `test_job_assignment_deserialize_with_null_commands` | `{"commands":null,...}` | Deserialization **fails** (type error) |
| `test_job_assignment_empty_commands_vec_rejected_at_execute` | `commands: vec![]` | Struct valid; `execute_job` rejects |
| `test_job_assignment_valid_image_and_commands_reaches_executor` | Valid image + non-empty commands | `execute_job` proceeds to executor |

## Files Changed

| File | Change |
|------|--------|
| `crates/gitforge-runner/src/agent.rs` | `execute_job`: change empty-commands from warn+proceed to error+reject; add 5 targeted tests |
| `docs/reports/test-results-20260828.md` | Updated with corrective pass results |
| `docs/reports/test-results-20260828.json` | Updated with corrective pass results |
