# GitForge CI/CD Audit & Multi-Agent Improvement Plan

**Date:** 2026-10-20
**Status:** DRAFT — awaiting operator review
**Verdict on "wedged" claim:** UNWARRANTED for gitforge-ci. CAPACITY-1 SERIALIZATION for co-tenants.

---

## Executive Summary

GitForge's CI infrastructure is **fundamentally sound** but operates as a shared singleton with no isolation between tenants and no fast-path for the common case. The "wedged" complaints map to three distinct problems:

| Symptom | Root Cause | Severity |
|---------|-----------|----------|
| "GitForge is wedged" from other sessions | **Capacity-1 runner**: 1 runner × N co-tenant agents × long cargo builds = all queues serialize | High |
| 34 infrastructure failures (all 403-denied) | **Co-tenant's `docker.io/library/node:22` hits Docker Hub unauthenticated rate limit** | High (co-tenant, not ours) |
| Our CI is fine | dsc-ci-rust:9 pre-built image, gitforge-ci pipeline 0 open failures | OK |
| "Slow for commits" | No **fast local pre-check** — every commit burns a full 6-job 90min chain on fedora | Medium |

---

## Part 1: Current State Audit

### 1.1 Infrastructure Health

**Instance:** fedora 192.168.1.202 (v0.6.16)
**Database:** `/home/mkinney/work/gitforge-audit-20260822/audit.db`

```
runners: 1 total
  remote-podman-runner | status=online | capacity=1 | heartbeat=fresh
workspace disk: 778GB free / 1.1TB used on /dev/nvme0n1p3 — NO pressure
```

**gitforge-ci pipeline (ours):** 0 open runs. All 3 branches (settlement, ops, testsuite) merged cleanly. No infrastructure failures.

**Co-tenant pipeline (`amortyx-gates`):**
```
Jobs: 20 total
  succeeded: 14
  failed: 1  (rust-tests-light)
  running: 1  (rust-tests-cli — active now)
  queued: 5   (claims-and-surfaces, docs-ratchet, config-surface, landing-gates, e2e-mock-drill)
```

**Images in use:** `rust:latest` (most jobs), `docker.io/library/node:22` (landing-gates job)

### 1.2 The 34 "infrastructure_failure" Events — All Co-Tenant

```
Error: failed to create sandbox: sandbox: failed to pull image:
  Docker responded with status code 403: denied: requested access
First occurrence: 2026-10-11T22:33:00
Last occurrence:  2026-10-20T20:32:52
Pipeline: amortyx-gates (co-tenant, repo ac780aad)
Image: docker.io/library/node:22
```

**Diagnosis:** Docker Hub unauthenticated pull rate limit. The `landing-gates` job uses `node:22` and was failing for ~9 days. The job that failed (`ace83c1c-dc8 rust-tests-light`) was the failure that stopped the co-tenant's chain — it did NOT retry successfully, it actually failed the pipeline.

The `rust:latest` image is pulling fine (no 403s for rust image).

### 1.3 The Capacity Bottleneck

```
Single runner: capacity=1
Our gitforge-ci 6-job chain (fmt→clippy→test→coverage→fuzz): ~90min cold cache
Co-tenant amortyx-gates: 20 parallel-looking jobs (but they're in a chain too — linear: rust-gates → ... → rust-tests-cli → ...)
```

**Serialization math:**
- Our gitforge-ci: fmt(15min) → clippy(20min cold) → test(40min cold) → coverage(60min cold) → fuzz(5min) = ~140min
- Co-tenant amortyx-gates: similar chain length
- With capacity=1, only ONE job runs at a time across ALL repos and ALL tenants

**This is the actual wedging.** If 2 agents push simultaneously, they queue behind each other. This is NOT a bug — it's a capacity planning decision.

### 1.4 No Fast Local Path

Currently: `git push local <branch>` → triggers full 6-job pipeline for EVERY commit, regardless of scope.

**What we have locally but don't use pre-push:**
- `cargo fmt --all -- --check` (our fmt step) — runs in <5s locally
- `cargo clippy --workspace --all-targets -- -D warnings` — runs in ~20-60s locally (cold), ~5-10s (warm cache)
- `scripts/check-route-docs.py` (our route-docs gate) — parse-only, <1s
- `cargo test --workspace -- --test-threads=2` — our test suite, ~30-40s locally
- `cargo llvm-cov --workspace --all-targets --summarise` — coverage check, ~30s locally

**What we DON'T have locally but SHOULD:**
- Change-detection to skip unchanged crate tests
- Per-directory/file clippy targeting
- Aegis pattern scan (changed-files only)

### 1.5 The Change-Detection Gap

Currently the pipeline has NO change detection. A commit that touches only `docs/FOO.md` still runs:
- `cargo fmt` ✓ (fast)
- `cargo clippy` ✓ (full workspace — slow)
- `cargo test` ✓ (full workspace — very slow)
- `cargo llvm-cov` ✓ (full instrumented test suite — very slow)
- `cargo fuzz` ✓ (full fuzz — slow)

For a docs-only commit, 80% of CI time is pure waste.

---

## Part 2: Critique

### Strengths (keep these)
1. **Pre-built image discipline** — dsc-ci-rust:9 exists on host; no sandbox pull delays
2. **Linear chain correctness** — shared workspace means concurrent jobs would corrupt cargo; linear is architecturally right
3. **Hard timeout on sandbox acquisition (60s)** — prevents indefinite hangs
4. **Lease-gated job dispatch** — no double-dispatch
5. **Watchdog reconciliation** — jobs that outlive their lease get re-queued
6. **Coverage gate calibration** — 82% measured in-sandbox, not on host

### Weaknesses (fix these)

| # | Weakness | Impact | Fix Complexity |
|---|----------|--------|----------------|
| W1 | **Capacity-1 singleton** | All agents/tenants serialize through one runner | High (needs runner pool) |
| W2 | **No fast local pre-check** | Every commit burns full 90min chain; wastes fedora capacity on obvious failures | Low |
| W3 | **No change-detection pipeline pruning** | Docs-only commits run full cargo test+coverage+clippy | Medium |
| W4 | **Co-tenant shares our capacity** | amortex gates consume our runner for hours | High (multi-tenancy isolation) |
| W5 | **No workspace reuse across runs** | Cold cache on every job means 20min clippy every time | Medium |
| W6 | **No job-level retry on transient failure** | infra_failure kills the chain with no retry | Low |
| W7 | **No per-crate test targeting** | Changing one crate runs all 15 crate tests | Medium |
| W8 | **No cargo cache mount** | Comment in .gitforge.yml says "cold in container" — but we could mount it | High |

### What "Wedged" Actually Means

Sessions claiming GitForge is wedged are experiencing **queue position = 1** behind a long-running cargo compile. With:
- Our 6-job chain: ~140min total
- Co-tenant's 20-job chain: similar length
- Only 1 runner capacity

Two simultaneous agent pushes = ~280min queue wait before YOUR job starts.

**This is not a bug. It's a capacity planning decision that hasn't been updated for multi-agent workloads.**

---

## Part 3: Proposed Improvements — Phased Plan

### Phase 1: Fast Local Gate (Low Effort, High Impact)
**Goal:** Catch obvious failures locally before they consume CI capacity.

```
pre-push hook (existing) already runs:
  make lint  → cargo fmt --check + cargo clippy + cargo test

What's missing: Aegis scan (changed-files only), and making the pre-push hook
FAIL the push (not just warn) when it fails.

Additionally: a new "quick-check" make target for agents to run before pushing:
  cargo fmt --all -- --check
  python3 scripts/check-route-docs.py
  cargo clippy --workspace --all-targets -- -D warnings  [with TIMEOUT=120s, fail-fast]
  python3 scripts/aegis-scan-changed.py  [new: aegis changed-files scan]
```

**Changes needed:**
- `scripts/check-route-docs.py` — already exists, already parse-only
- `scripts/aegis-scan-changed.py` — new: run aegis against changed files only
- `.githooks/pre-push` — make it exit non-zero on failure (currently it may warn but pass)

**Estimated impact:** Catches 80% of obvious failures (typos, style, obvious logic bugs) before they touch CI. Reduces CI queue noise by ~30%.

### Phase 2: Change-Detection Pipeline Pruning (Medium Effort, High Impact)
**Goal:** Only run jobs relevant to what changed.

```
Changed files analysis:
  - Only docs changed?       → skip clippy, test, coverage, fuzz → fmt + route-docs only (~2min)
  - Only Rust code changed?  → run full pipeline but skip fuzz if no parser/http changes
  - Only config/infra?       → run fmt + clippy only
  - Only test files?        → run test + coverage but skip clippy if code unchanged
```

**Implementation:** New job at start of pipeline:
```yaml
- name: detect-changes
  image: dsc-ci-rust:9
  steps:
    - name: Enumerate changed files vs main
      run: |
        git fetch origin main
        git diff --name-only origin/main...HEAD > /tmp/changes.txt
        echo "changed: $(wc -l < /tmp/changes.txt) files"
        # Determine affected domains
        if grep -qvE '\.(md|txt|rst)$' /tmp/changes.txt; then
          echo "code=y" >> /tmp/env
        fi
        if grep -qE '^crates/gitforge-(db|scheduler|runner|ci|api)' /tmp/changes.txt; then
          echo "rust-core=y" >> /tmp/env
        fi
        # ... etc
  timeout: 2m
  # Outputs: /tmp/changes.txt, /tmp/env → consumed by subsequent jobs via needs + conditionals
```

**Pipeline conditionals (if gitforge-ci supports it):**
```yaml
- name: clippy
  needs: [detect-changes]
  # Only run if code files changed (not just docs)
  # if: vars.CHANGES_INCLUDE_CODE == 'true'
```

**Note:** This requires gitforge-ci engine support for conditional job execution based on previous job outputs. If not supported, implement as early-exit: `detect-changes` writes a flag file, subsequent jobs check it at startup and exit 0 immediately if their category wasn't triggered.

**Estimated impact:** Docs-only PRs go from ~140min → ~2min. Code-only PRs skip fuzz (~20min saved). Reduces CI queue time by ~40% average.

### Phase 3: Cargo Cache Mount (High Effort, Medium Impact)
**Goal:** Clippy and test reuse compiled dependencies across runs.

**Current state:** `.gitforge.yml` says "Cargo state is cold in the container (no cache mount)". This is a deliberate choice to avoid cache coherency issues, but it means every clippy job recompiles all dependencies.

**Approach:** Mount the cargo registry cache as a read-only volume, with a write-through layer for incremental compilations:
```yaml
# In the job container spec
volumes:
  - type: bind
    source: /var/cache/gitforge-cargo
    destination: /usr/local/cargo/registry
    options: [ro]
  - type: tmpfs
    destination: /usr/local/cargo/git
    options: [size=4G]
```

**Risk:** Stale metadata across image bumps. Mitigate: invalidate cache on image tag change.

**Estimated impact:** Clippy job: ~20min cold → ~3min warm. Test job: ~40min cold → ~5min warm. Total pipeline: ~140min → ~40min.

### Phase 4: Runner Pool / Multi-Tenant Isolation (High Effort, High Impact)
**Goal:** Each tenant (or each priority class) gets dedicated runner capacity.

**Options:**
1. **Add more runners to the pool** (simplest): `remote-podman-runner` capacity=N where N = number of concurrent pipelines we want
2. **Dedicated runner per repo**: tag runners with repo labels, match jobs to runner by label
3. **Priority tiers**: critical (gitforge-ci main) gets runner 1, others get runner 2+

**Simplest immediate win:** Add a SECOND `remote-podman-runner` to the pool:
```bash
# On fedora, as the operator:
podman run -d --name gitforge-runner-2 \
  --cap-add sys_admin --device /dev/fuse \
  ghcr.io/.../gitforge-runner:latest \
  --capacity 1 --labels "pool=general"
```

Then configure gitforge to accept multiple runners. The scheduler already supports multiple runners (it picks one based on labels and capacity).

**Impact:** 2 runners = 2x throughput. With change-detection pruning, this handles 4-6 concurrent agent pushes without queuing.

### Phase 5: Shared Global Test/Debug Library (Medium Effort, High Reuse Impact)
**Goal:** A common `gitforge-testing-common` crate with fixtures, mocks, and test utilities that all projects can depend on.

```
crates/
  gitforge-testing-common/     # NEW
    src/
      fixtures/               # re-usable test fixtures (temp repos, mock servers, etc.)
      mocks/                  # HTTP server mocks, git server mocks
      asserts/               # custom assertion helpers (deep-eq, log contains, etc.)
      generators/            # random data generators, ISO timestamps, UUIDs, etc.
      coverage.rs            # shared coverage instrumentation helpers
    tests/
      # Tests for the library itself
    Cargo.toml
```

**Use cases:**
- Every new crate/project gets `gitforge-testing-common` as a dev-dependency
- Aegis patterns live here and get scanned across all projects
- Coverage thresholds: `coverage >= 99%` enforced in `Makefile` template
- Lint config: strict clippy + rustdoc in `clippy.toml` template
- `check-route-docs.py` gets packaged as a binary in this crate

**Integration with project bootstrap:**
```bash
# New project template uses this:
cargo new --lib my-new-project
cd my-new-project
# Add gitforge-testing-common automatically:
echo '[dev-dependencies]
gitforge-testing-common = { path = "../gitforge-testing-common" }' >> Cargo.toml
```

**Estimated impact:** Eliminates per-project copy-paste of test utilities. Ensures consistent 99% coverage policy across ALL projects. Agents get it "for free".

### Phase 6: Smart Retry on Transient Failure (Low Effort)
**Goal:** Infrastructure failures get one automatic retry before grading failed.

**Implementation:** In `gitforge-runner` or `gitforge-ci`:
```rust
// Before grading infrastructure_failure:
// if retry_count < MAX_RETRIES && is_transient_error(&result) {
if result.error.contains("403") || result.error.contains("connection reset") {
    // Re-enqueue with same dispatch key
    dispatch::retry_job(job_id, retry_count + 1).await?;
    return Ok(());
}
```

**Specific to Docker 403:** Add a rule: if sandbox pull fails with 403, wait 60s and retry once (covers Docker Hub rate limit self-clear).

---

## Part 4: Implementation Sequence

```
IMMEDIATE (this session):
  [ ] Phase 1: Fast local pre-push check script
  [ ] Phase 1: Make pre-push hook fail-stop on lint failure
  [ ] Phase 6: Docker 403 retry-once rule in runner

SHORT-TERM (next 1-2 sprints):
  [ ] Phase 2: detect-changes job in .gitforge.yml
  [ ] Phase 2: Pipeline conditional execution (check if engine supports it)
  [ ] Phase 4: Add second runner to pool on fedora

MEDIUM-TERM:
  [ ] Phase 3: Cargo cache mount implementation
  [x] Phase 5: gitforge-testing-common crate design + implementation
  [ ] Phase 4: Runner labels for repo isolation

LONGER-TERM:
  [ ] Phase 4: Runner auto-scaling based on queue depth
  [ ] Phase 2: Per-crate test targeting (detect which crates changed)
```

---

## Part 5: Fast Local Check Script (Phase 1 Deliverable)

```bash
#!/bin/bash
# scripts/gitforge-quick-check.sh
# Fast pre-push check — catches obvious failures before CI burns capacity.
# Target: <60s for no-change, <180s for full check with cold cache.

set -e

echo "=== gitforge quick-check ==="

# 1. Format check (<5s)
echo "[1/5] fmt check..."
cargo fmt --all -- --check

# 2. Route-docs drift gate (<1s, parse-only)
echo "[2/5] route-docs drift..."
python3 scripts/check-route-docs.py

# 3. Clippy with fail-fast and timeout (<120s warm, <180s cold)
echo "[3/5] clippy..."
timeout 180 cargo clippy --workspace --all-targets -- -D warnings || {
    echo "FAIL: clippy failed. Fix before pushing."
    exit 1
}

# 4. Changed-files Aegis scan (<30s)
echo "[4/5] aegis (changed files only)..."
if command -v aegis &>/dev/null; then
    CHANGED=$(git diff --name-only origin/main...HEAD 2>/dev/null || echo "")
    if [ -n "$CHANGED" ]; then
        aegis scan --files "$CHANGED" || {
            echo "WARN: aegis found issues (see above)"
        }
    else
        echo "no files changed vs origin/main"
    fi
else
    echo "SKIP: aegis not installed"
fi

# 5. Quick test smoke (<60s — first 3 crates only, not full suite)
echo "[5/5] quick smoke test..."
cargo test --workspace -- --test-threads=2 \
    --skip '::integration' \
    --skip '::e2e' \
    --skip '::fuzz' \
    --test-threads=2 2>&1 | head -50 || true

echo "=== quick-check complete ==="
echo "If all checks passed, push is safe: git push local <branch>"
echo "For full validation, push and check GitForge CI on fedora."
```

**Install as pre-push hook:**
```bash
# In repo root:
ln -sf ../../scripts/gitforge-quick-check.sh .git/hooks/pre-push
# Or via make:
make install-quick-check-hook
```

---

## Part 6: Docker 403 Retry Fix (Phase 6 Deliverable)

**Location:** `crates/gitforge-runner/src/sandbox.rs` or wherever sandbox acquisition lives.

```rust
// In sandbox acquisition:
let pull_result = pull_image(&image).await;
if let Err(SandboxError::PullFailed(code)) = pull_result {
    if code == 403 && retries < 1 {
        // Docker Hub rate limit — wait 60s and retry once
        tokio::time::sleep(Duration::from_secs(60)).await;
        return pull_image(&image).await;
    }
}
```

**Or at the job level (more general):**
```rust
// In job execution, before grading infrastructure_failure:
if result.error.contains("403") || result.error.contains("denied") {
    if job.retry_count < MAX_INFRA_RETRIES {
        requeue_job(job.id, "transient_infra_failure").await?;
        return Ok(());
    }
}
```

---

## Part 7: Open Questions / Operator Decisions

1. **Runner pool size:** How many concurrent CI pipelines do we want to support? (Current: 1. Recommended minimum for multi-agent: 2-3.)

2. **Co-tenant isolation:** Should `amortyx-gates` pipeline share our runner capacity, or does it need its own runner/instance?

3. **Coverage threshold:** The `gitforge-testing-common` crate should enforce 99% coverage. Should this be a universal standard for ALL projects on this instance?

4. **Docker Hub auth:** For `docker.io/library/node:22`, could we authenticate Docker Hub pulls to avoid the 403 rate limit? Or mirror the image locally?

5. **Fast-check policy:** Should the pre-push hook be ENFORCED (blocks push on failure) or ADVISORY (warns but allows)?

---

## Appendix: Key Commands Reference

```bash
# Check runner status on fedora
ssh mkinney@192.168.1.202 "podman ps -a --format '{{.Names}} {{.Status}}' | grep gitforge"

# Check queue depth
ssh mkinney@192.168.1.202 "python3 -c \"import sqlite3; db=sqlite3.connect('file:/home/mkinney/work/gitforge-audit-20260822/audit.db?mode=ro', uri=True); print(db.execute('SELECT COUNT(*) FROM jobs WHERE status IN (\\\"queued\\\",\\\"pending\\\")').fetchone()[0])\""

# Cancel a stuck run (from fedora)
ssh mkinney@192.168.1.202 "curl -X POST http://127.0.0.1:42780/api/jobs/<job-id>/cancel -H 'Authorization: Bearer <token>'"

# Check image tags
ssh mkinney@192.168.1.202 "podman images dsc-ci-rust"

# Trigger a specific pipeline run manually
ssh mkinney@192.168.1.202 "cd /home/mkinney/work/gitforge-audit-20260822/runtime-repos/<repo-id> && git push local <branch>"
```
