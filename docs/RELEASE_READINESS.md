# Release readiness and operational evidence

This document is an evidence checklist, not a declaration that every subsystem
has passed production qualification. A capability should be described as
production-ready only when its acceptance evidence is reproducible against a
tagged release and the supported deployment topology.

## Current evidence boundary

The repository README documents the intended service topology, ports,
environment variables, Docker Compose deployment, administrator bootstrap,
runbook, API, and test strategy. This page does not independently establish
that every deployment, security boundary, recovery path, or scale target has
been exercised. Record dated evidence for each row before upgrading its status.

## Subsystem qualification matrix

| Subsystem | Evidence required before calling it production-qualified | Current status in this checklist |
|---|---|---|
| Git over SSH and HTTP | Clone, fetch, push, branch/tag operations, large packfiles, interrupted transfers, authz, and concurrent access tested against release build | Not independently audited |
| Authentication and authorization | Bootstrap, login, token expiry/revocation, permission boundaries, tenant/repository isolation, and negative tests | Not independently audited |
| API gateway | OpenAPI/route contract tests, request limits, error semantics, rate limits, and compatibility tests | Not independently audited |
| CI scheduler | Durable enqueue/claim/lease/heartbeat/retry/cancel behavior; crash recovery; duplicate delivery; queue saturation | Not independently audited |
| Runner isolation | Container privilege review, filesystem/network boundaries, resource limits, secret handling, cleanup, and hostile-job tests | Not independently audited |
| Artifacts and caches | Integrity checks, access control, retention, quota enforcement, cache poisoning tests, and cleanup | Not independently audited |
| Persistence and migrations | Upgrade from prior supported release, interrupted migration behavior, rollback/forward-recovery, and backup compatibility | Not independently audited |
| Backup and restore | Documented backup procedure plus restore drill into a clean environment; measured RPO/RTO | Not independently audited |
| Observability | Health/readiness semantics, actionable metrics, structured logs, trace correlation, alert thresholds, and failure injection | Not independently audited |
| Upgrade and rollback | Signed/checksummed release artifacts, configuration compatibility, migration plan, rollback limitations, and operator runbook | Not independently audited |
| High availability | Explicitly supported topology, failover behavior, shared-state consistency, and recovery tests; otherwise document as unsupported | Not independently audited |

## Release evidence record

For each release candidate, attach or link:

1. Exact commit SHA, tag, build provenance, and artifact checksums.
2. Supported OS/architecture and deployment topology.
3. CI run URLs and test summaries for unit, integration, end-to-end, security,
   and migration tests.
4. A clean-install deployment record using only documented configuration.
5. A clone/fetch/push test using both supported Git transports.
6. A CI job lifecycle test covering success, failure, cancellation, retry,
   worker loss, and scheduler restart.
7. Runner isolation test results and a review of secrets exposure paths.
8. Backup/restore drill results, including measured recovery time and data loss.
9. Known limitations, unsupported topologies, and rollback constraints.

## Recommended qualification sequence

1. **Single-node baseline:** clean install, bootstrap, login, repository
   creation, clone/fetch/push, simple CI job, artifact retrieval.
2. **Failure behavior:** kill/restart API, scheduler, and runner processes at
   different points in a job; verify durable state and safe retries.
3. **Isolation:** run untrusted jobs and validate privilege, filesystem,
   network, CPU, memory, and secret boundaries.
4. **Recovery:** back up a populated instance, restore into a clean host, and
   verify repositories, users, settings, artifacts, and scheduler state.
5. **Upgrade:** upgrade from the previous supported release with a real
   production-shaped dataset; document rollback boundaries.
6. **Scale and HA:** only claim support after load and failover tests against
   the exact documented topology.

## Status vocabulary

- **Not independently audited:** documentation or code exists, but this
  qualification pass has not verified runtime behavior.
- **Verified for release X:** evidence is linked and reproducible for a named
  release and topology.
- **Unsupported:** the capability is intentionally not provided.
- **Planned:** implementation or qualification work remains.

Do not use a generic green "production-ready" label to imply that every row
above has passed. State the specific release, topology, and verified
capabilities instead.
