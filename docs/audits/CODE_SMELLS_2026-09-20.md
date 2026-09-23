# Code-Smell and Strict-Lint Audit — 2026-09-20

Scope: entire Rust workspace (16 crates, 4 service binaries) under
`cargo clippy --workspace --all-targets -- -D warnings`, a manual
unwrap/panic survey of all non-test library code, and a clippy-pedantic
distribution survey (`-W clippy::pedantic`).

## Fixed in this pass

| Smell | Location | Fix |
|---|---|---|
| Infallible `Result` dispatch | `gitforge-api/routes/ci.rs` | `claims_from_user` always returned `Ok(user.claims)`; five handlers matched on it with unreachable `Err` arms. Inlined `let claims = user.claims;` (net −17 lines). |
| Derive-macro test bloat | `gitforge-api/routes/ci.rs`, `runners.rs` | 48 tests asserted "serde can serialize a struct I just built" (field-echo). Replaced with two wire-contract tests asserting exact JSON. The new test immediately caught that `RunnerResponse` serializes `runner_type` as `"type"` (serde rename) — a fact no old test pinned. |
| Panic paths in request handlers | `services/git-server/main.rs` | 22 `Response::builder().unwrap()` chains replaced with `finish_response`, degrading to a bare 500 response instead of unwinding the connection thread. |
| Inconsistent formatting/Option style | workspace | `uninlined_format_args`, `map_unwrap_or`, `redundant_closure_for_method_calls`, `ignored_unit_patterns` promoted to `deny` in `[workspace.lints.clippy]`; all 19 members opt in via `[lints] workspace = true`. ~90 mechanical fixes applied. |

## Unwrap survey (130 sites in non-test code)

| Class | Count (approx) | Verdict |
|---|---|---|
| Literal `Regex::new(...).unwrap()` | ~60 | Accepted. Compile-time constants; an invalid literal is a programmer error that unit tests catch. |
| `Mutex::lock().unwrap()` | ~30 | Accepted. Standard-library poisoning idiom; poisoning already implies a panicked peer. |
| Startup metric registration `.expect(msg)` | ~32 (`metrics.rs`) | Accepted. Runs once at process init with static names; failure is a build-time misconfiguration. |
| Top-level `main` bootstrap | ~8 | Accepted by project convention (`panic` ban excludes top-level `main`). |
| Test-module occurrences | excluded from count | n/a |

## Pedantic survey (548 findings, advisory)

Dominant classes and why they stay advisory for now:

- `cast_possible_truncation` (45): deliberate narrowing in protocol/size
  code (byte lengths, capacities). Fixing means auditing every cast for
  range guards — worthwhile, but a correctness campaign of its own.
- `doc_markdown` (26), `missing_errors_doc` (17), `must_use_candidate`
  (19): documentation polish; batched with the docs audit rather than
  spread across code commits.
- `too_many_lines` (13): handler-length refactors; each needs a
  behavioral test harness first to be safe.
- Remainder (<10 each): `similar_names`, `return_self_not_must_use`,
  `needless_pass_by_value`, etc.

Four mechanical classes were promoted to `deny` (see above); the rest is
re-reviewable when the doc/cast campaigns land.

## Follow-ups

1. Enable `cast_possible_truncation` crate-by-crate with explicit range
   guards (start with `gitforge-common`, `gitforge-events`).
2. Split the 13 `too_many_lines` handlers once route-level fault
   injection (see IMPROVEMENTS.md follow-ups) makes them testable.
3. Convert literal regexes in `gitforge-review` to `OnceLock` to build
   them once instead of per call.
