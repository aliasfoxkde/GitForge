# Security Audit Receipt — 2026-11-02

Three parallel audits (secrets exposure, CI sandbox isolation, OpenCodifier
quality) with adversarial verification of the headline claims. Raw finding
files: `/var/tmp/gitforge-audit/{secrets,sandbox}-findings.md`.

## Verified and fixed this session

### F-06 (was MEDIUM-HIGH): pipeline-authored bind mounts reached any host path
`volumes[].source` in `.gitforge.yml` was accepted with only an
absolute-path check; `build_volume_mounts`
(`crates/gitforge-sandbox/src/docker.rs`) bound any host path read-write
into the job container. `source: /var/lib/gitforge` (holds `jwt.secret`) or
`source: /` mounted rw. **Fixed 2026-11-02**: bind sources are now
allowlisted against the cache root + `GITFORGE_BIND_ROOTS` (falling back to
`GITFORGE_WORKSPACE_ROOT(S)`); out-of-policy sources are skipped with a
WARN and never mounted. Pinned tests:
`test_build_volume_mounts_skips_bind_outside_trusted_roots`,
`test_build_volume_mounts_allows_explicit_bind_roots`.

### F-04 (CRITICAL): pipeline `working_directory` mounted any host dir at /workspace
The YAML value flowed unvalidated into `create_with_workspace`, which only
checked `is_absolute() && is_dir()`. **Fixed 2026-11-02**: the workspace is
canonicalized and must resolve under a trusted root, else the sandbox
refuses to create the container. Cannot degrade silently — the workspace IS
the job's filesystem.

Deployment note: no pipeline in the live store uses explicit `volumes:`
sources or step `working_directory` (verified via SQL over
`pipelines.config`); the only cache blocks use `{cache_root}` placeholders,
which pass the allowlist unchanged.

## Verified FALSE (audit correction)

### "Live 64-char JWT_SECRET in public GitHub history" — NOT TRUE
The only JWT_SECRET ever committed on any ref is the placeholder
`dev-secret-change-in-prod-min-32-chars!!` (40 chars, sha256 f929c3a9f081).
A blob scan of all 17,532 fetched objects finds the live 64-char secret in
zero blobs. The public branch `fix/gitforce-delta-doctest-20260822` does
carry `.env`, but only with placeholder values. The exposure-critical
severity does not survive verification; branch hygiene (untracking `.env`
on the public mirror) remains low-priority cleanup.

## Confirmed, open (in task ledger / findings files)

- **F-01/F-02/F-14 (HIGH)**: no `cap_drop`, `no-new-privileges`,
  `pids_limit`, `user:` pinning, `readonly_rootfs`; jobs run as image
  default (root) with the default Docker cap set incl. NET_RAW.
- **F-05 (HIGH)**: operator API `/jobs` submit persists caller-supplied
  `working_dir` with only length validation. Mitigated in part by the
  sandbox-side workspace root check above (the mount now refuses), but the
  control plane should validate at admission too.
- **F-12 (MEDIUM)**: host-side `git checkout --force` adopts a
  container-writable tree — plausible hook/config exec vector.
- **F-16 (MEDIUM)**: no registry allowlist/digest pinning for job images.
- **S9 (MEDIUM)**: job logs world-readable (0644) under
  `artifacts/job_logs/`; no redaction pass over persisted stdout.
- **/etc/gitforge/database.env 0644** — sqlite path only, no secret; tighten
  to 0640 anyway.
- Queue capacity: single online runner (swarmone-docker, capacity 1);
  `fedora-docker` (capacity 5) offline since 2026-10-22. Six same-night
  pushes to backend-fixed produced six full 16-job pipelines — no per-branch
  supersede policy exists (webhook idempotency keys on commit hash only).
  Feature gap: newest-SHA-wins cancellation per (repo, branch, pipeline).

## Infrastructure actions this session

- `dsc-ci-rust:16` built on Fedora from HEAD (ce06089e) per the
  storage-placement rule (docker builds belong on Fedora), transferred and
  loaded on SWARMONE. This unblocked GitForge self-CI, which had been red
  since 2026-11-01 09:10 UTC: every run died in `detect-changes` with
  "pull access denied for dsc-ci-rust" because the pipeline pins tag 16 and
  the host only had 14.
- Rerun triggered for ae22006b via `/api/pipeline-runs/{id}/rerun`.
