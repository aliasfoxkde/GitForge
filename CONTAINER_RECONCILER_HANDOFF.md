# GitForge abandoned-container reconciler handoff — 2026-09-14

## Objective

Add a fail-closed reconciler for Docker/Podman containers created by GitForge
jobs whose runner or attempt disappeared before the normal sandbox destroy
path ran. The reconciler must be safe under concurrent jobs, restartable, and
observable through a durable receipt.

This is a GitForge implementation task. Do not modify the separately managed
GitForge checkout from Platform-Architecture.

## Evidence that opened the task

On Fedora `mkinney@192.168.0.201`, the live system runner was healthy:

```text
gitforge@runner.service: ActiveState=active, NRestarts=0, TasksCurrent=9
```

The three ignored `gitforge-sandbox` integration tests were run against Docker
`26.1.5+dfsg1` and all passed. Immediately afterward, four containers still
existed with both `com.gitforce.managed=true` and `com.gitforce.job_id` labels;
they were exited and 28–30 hours old. Exact IDs are retained in
`docs/progress/FEDORA_GITFORGE_DOCKER_INTEGRATION_RECEIPT_2026-09-14.md`.

The current `DockerSandbox::remove_job_containers(job_id)` cleanup is scoped to
one known job ID. It cannot discover an attempt whose queue row or runner
process was lost.

## Existing implementation to preserve

Read-only history audit found these relevant changes already present on
GitForge `origin/main` at `235292df`:

- `d4c2e027` reaps containers left by an abandoned acquisition timeout and
  improves durable runner logging.
- `020eab0c` addresses workspace-sweep and stale-container removal races.
- `8a8aa7be` tolerates Docker 404/409 races during stale-job cleanup.
- `235292df` force-removes owned containers during normal teardown.

These commits are not sufficient evidence for the current gap: the deployed
runtime still contained four exited, GitForge-labeled containers from lost or
historical attempts. The new reconciler must complement these paths rather
than replace or duplicate them.

## Required design

1. **Ownership:** select only containers with the exact managed label
   `com.gitforce.managed=true`; require a valid GitForge job/attempt label. Do
   not infer ownership from names, image names, age, or exit code.
2. **State correlation:** reconcile container labels against authoritative
   scheduler/queue state and attempt identity. An active, claimed, or recently
   updated job must never be removed.
3. **Grace period:** require a configurable, positive age threshold after
   container exit and document its default. Never remove a running container
   merely because its metadata is old.
4. **Concurrency:** use an idempotent remove operation and tolerate Docker
   404/409 races. A concurrent normal teardown must be a successful no-op.
5. **Dry run:** provide a read-only census that emits candidate IDs, labels,
   state, age, reason, and the decision (`retain` or `eligible`), without any
   remove call.
6. **Deletion authority:** deletion must be explicitly enabled by a service or
   command policy; default startup behavior should be census-only until the
   policy is qualified.
7. **Receipt:** record counts and container IDs/hashes, not environment values,
   tokens, or full logs. Include runtime, policy, queue snapshot hash, and
   removal outcomes.
8. **Recovery:** run the reconciler at runner startup and after a supervisor
   interruption, plus a bounded periodic schedule. It must not block normal
   job admission for an unbounded Docker API call.

## Source seams to inspect

- `crates/gitforge-sandbox/src/docker.rs`: labels, `remove_job_containers`,
  create/destroy paths, and Docker error handling.
- `crates/gitforge-runner/src/agent.rs`: startup, job assignment, sandbox
  ownership, and cancellation paths.
- Scheduler/queue models: authoritative claim, lease, completion, and attempt
  identity. Do not treat a missing row as proof that a job is abandoned until
  retention rules are explicit.
- Service/systemd configuration: timeout, resource, and output policy for the
  reconciler.

## Required tests

- Unit tests for label filtering, malformed/missing labels, age/grace policy,
  active-job retention, unknown-job retention, dry-run no-mutation, and 404/409
  idempotency.
- Mock Docker API test proving only exact GitForge labels are selected.
- Disposable Docker integration test: create a labeled container, simulate an
  abandoned attempt, verify dry-run candidate, enable deletion, verify the
  container is absent, and prove an unrelated labeled/non-GitForge container
  remains.
- Restart test: run reconciler twice and prove the second run is idempotent.
- Resource test: per-call timeout and bounded candidate count.

## Acceptance gates

The task is complete only when all are true:

- `cargo fmt --all -- --check`, workspace tests, strict Clippy, and relevant
  integration tests pass.
- A real disposable Docker/Podman canary proves candidate discovery and
  deletion, with before/after container census and no unrelated deletion.
- Runner termination followed by reconciler startup leaves zero eligible
  disposable containers and no active-job container removed.
- Aegis/security scans introduce no unreviewed finding.
- GitForge receipt contains exact revision, policy, candidate decisions,
  outcomes, and cleanup proof.
- Platform-Architecture `CURRENT_STATE.md` links the receipt and keeps any
  unproven production claims explicitly partial.

## Non-goals

Do not globally prune Docker, remove containers by name prefix alone, delete
the four historical containers before retention/ownership review, loosen
resource limits, or claim that systemd `KillMode=control-group` controls
containers created through a separate Docker daemon cgroup.
