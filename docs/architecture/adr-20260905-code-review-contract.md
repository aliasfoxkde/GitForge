# ADR: Code Review Contract — Run and Finding Invariants

**Status:** Accepted — 2026-09-05

## Context

GitForge will execute AI code review on repository changes and emit findings as
inline comments on commits, pull requests, or merge requests.  Before any
execution, webhook handler, database migration, or provider-specific behavior
is implemented, the run lifecycle and finding shape must be frozen so that all
consumers share the same immutable contract.

## Invariants

### R1 — Immutable head SHA

A review run is anchored to a single commit SHA (`head_sha`).  The SHA
supplied at submission time is the authoritative HEAD for the entire run.
The run must not re-base, fetch, or advance to a different SHA during
execution.  If the caller supplies a SHA that is not available locally, the
run fails with a transient error; it does not attempt to fetch or resolve the
reference.

### R2 — Idempotency key

Every review run submission carries a caller-supplied `idempotency_key`
(maximum 128 bytes, non-empty string).  The scheduler persists the key
against the run ID.  Re-submitting the same key for the same `head_sha`
returns the original run ID with no new run created.  Re-submitting the same
key for a *different* `head_sha` returns `409 Conflict`.  The scheduler is
the durable authority; in-process mirrors do not substitute for persistence.

### R3 — Monotonic terminal states

A review run transitions through:

```
pending → running → succeeded | failed | cancelled
```

Terminal states (`succeeded`, `failed`, `cancelled`) are final.  No
transition path returns a terminal run to a non-terminal state.  State
transitions use conditional updates so that a competing scheduler or stale
runner cannot retroactively change an outcome.

### R4 — Stable finding fingerprints

Every finding carries a deterministic, content-addressable `fingerprint`
computed from:

- `file` — the file path relative to the repository root
- `line` — the 1-based line number, or `null` if the finding is file-level
- `rule` — the identifier of the rule that triggered the finding
- `message` — the exact content string emitted by the model

The fingerprint is a single stable string (e.g., SHA-256 digest truncated to
16 hex digits).  Two findings with identical fields produce the same
fingerprint.  Fingerprints enable deduplication across retried runs on the
same SHA.

### R5 — Explicit position status

Every finding has an explicit `position_status` drawn from an enumerated set:

| Value | Meaning |
|-------|---------|
| `line` | Finding applies to a specific line (both `file` and `line` are set) |
| `file` | Finding is file-level; `line` is `null` |
| `deleted` | Finding pointed to a line that was deleted in the current diff; the finding is retained with `line = null` |
| `unavailable` | Finding position cannot be resolved (e.g., the file was renamed); it must not be posted as an inline comment |

`null` for `line` is only permitted when `position_status` is `file`,
`deleted`, or `unavailable`.  No other combination is valid.

### R6 — Invalid model output never becomes an inline comment

When the model's raw output cannot be parsed into a well-formed finding
(e.g., malformed JSON, missing required fields, out-of-range line number
with no `position_status` other than `line`), the finding is discarded and
replaced by an internal sentinel finding with `position_status =
unavailable`.  The run transitions to `failed` only when the failure is
permanent (e.g., the provider is unreachable); parse failures alone do not
cause failure, but they produce no user-visible inline comments.  The
sentinel finding is stored for audit purposes and is never posted to the
platform.

## Consequences

- Any consumer of the review run or finding API can rely on the above
  invariants without inspecting provider-specific internals.
- Run deduplication (R2) and state monotonicity (R3) are enforced by the
  scheduler's durable store; in-process caches are advisory only.
- Finding stability (R4) and explicit position (R5) allow platform
  integrations (GitHub, GitLab) to map findings to their comment APIs without
  guessing or performing heuristic line matching.
- R6 guarantees that a misbehaving or hallucinating model cannot inject an
  arbitrary string into a commit or PR as a comment.
