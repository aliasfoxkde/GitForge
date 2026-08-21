# GitForge CI Audit

**Date:** 2026-08-21
**Scope:** `.github/workflows/gitforge-ci.yml` and the active workflow set

---

## Observed Evidence

### 1. `gitforge-ci.yml` duplicates `rust-ci.yml` trigger paths

Both workflows trigger on the identical set of paths:

```yaml
# gitforge-ci.yml lines 8-12
paths:
  - '**.rs'
  - '**/Cargo.toml'
  - '**/Cargo.lock'
  - '.github/workflows/**'
```

```yaml
# rust-ci.yml lines 8-12
paths:
  - '**.rs'
  - '**/Cargo.toml'
  - '**/Cargo.lock'
  - '.github/workflows/rust-ci.yml'
```

On every Rust-related push/PR, **both** workflows run. `gitforge-ci.yml` enqueues a remote build to GitForge; `rust-ci.yml` runs `fmt`, `clippy`, `test`, `coverage`, `security`, and `build` jobs **directly on GitHub Actions runners**. The comment at line 21-22 of `gitforge-ci.yml` states the intent to route *all* builds through GitForge to prevent build storms, but `rust-ci.yml` is still active and unconditionally runs the same work on GitHub runners — defeating that purpose.

### 2. `gitforge-ci.yml` has no `concurrency` control

`gitforge-ci.yml` lacks a `concurrency` block. Every push to `main`/`stable` or every PR update enqueues a new build and spawns a fresh polling loop, with no cancellation of previous runs. By contrast, `rust-ci.yml`, `always.yml`, `security.yml`, `integration.yml`, and `auto-merge.yml` all define `concurrency` groups with `cancel-in-progress: true`.

### 3. Polling loop can silently succeed on non-terminal states

In `gitforge-ci.yml` lines 113-119, the `case` statement exits with `0` for `completed|succeeded|success|passed` but exits with `1` for all other terminal states (`failed`, `cancelled`, `error`, `timeout`). However, if GitForge returns an **unrecognised** status string (e.g., a typo like `sucess` or a new state like `queued` that isn't in the terminal list), the `*)` branch simply sleeps and polls again — eventually timing out after 1800s. The error message on timeout says "did not reach a terminal state" rather than surfacing the unexpected status value, making diagnosis harder.

### 4. `auto-merge.yml` has a 60-second race window

`auto-merge.yml` line 31-44 polls CI status for at most 60 seconds (12 iterations × 5s). On PR events, `gitforge-ci.yml` enqueues to GitForge then polls for up to 1800 seconds. The auto-merge workflow can observe a transiently passing state from `always.yml` (which always passes) or stale prior-run statuses before the GitForge poll job finishes, and proceed to merge — **before** the actual Rust CI gate has reported its final result.

### 5. `security.yml` `dependency-review` uses `continue-on-error: true`

Line 79 of `security.yml` sets `continue-on-error: true` on the dependency review action, meaning vulnerable dependency introductions will never block a merge.

---

## Prioritized Reliability Finding

### **CRITICAL — `rust-ci.yml` and `gitforge-ci.yml` run in parallel with no coordination, creating divergent CI signals and wasted runner spend**

**Severity:** High
**Impact:** Reliability, cost, and correctness of the CI gate.

**Detail:** The stated design intent in `gitforge-ci.yml` (lines 21-22) is to route *all* builds through GitForge's queue. In practice, `rust-ci.yml` continues to run the full suite (6 parallel jobs: fmt, clippy, test, test-race, coverage, security) on GitHub Actions runners on every triggering event. This means:

1. **Double spend:** Every Rust change consumes GitHub Actions runner minutes *and* GitForge queue capacity simultaneously.
2. **Conflicting signals for auto-merge:** The `auto-merge.yml` workflow reads combined CI status after only a 60-second wait. `always.yml` passes immediately; `rust-ci.yml` may still be running; `gitforge-ci.yml` is polling for up to 30 minutes. Auto-merge can fire on incomplete information.
3. **No single source of truth:** If `rust-ci.yml` passes but GitForge fails (or vice-versa), there is no defined resolution — both report as separate workflow conclusions to the branch protection check.

---

## Minimal Recommendation

**Disable `rust-ci.yml` triggers when `gitforge-ci.yml` is active** (or vice-versa) so that exactly one workflow owns the Rust CI gate.

The smallest change: add a repository variable `GITFORGE_ENABLED` (defaulting to `false` for backward compatibility) and gate `rust-ci.yml`'s jobs on it being unset, while gating `gitforge-ci.yml`'s `enqueue-build` job on it being `true`.

Alternatively, if `gitforge-ci.yml` is not yet production-ready, the immediate mitigation is to **add a `concurrency` group to `gitforge-ci.yml`** matching the pattern used by all other workflows:

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true
```

This alone prevents unbounded parallel polling loops from accumulating on rapid pushes, and is a one-line, zero-risk change.

---

*Audit completed by ZAI. Findings are based on static inspection of workflow files in this worktree.*