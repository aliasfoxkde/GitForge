# GitForge Fedora user-systemd policy

These files describe the active **user-systemd deployment contract** for the
Fedora host. The running services are `gitforge-api.service`,
`gitforge-ci.service`, `gitforge-git-server.service`, and
`gitforge-runner.service`; their installed units and operational drop-ins are
under `~/.config/systemd/user/`. The system-scope template
`systemd/gitforge@.service` is not the active deployment. A names-only audit
on 2026-10-05 found provider credentials in the active processes; the
isolation drop-in remains uninstalled and requires a drained-queue rollout
with rollback. See
`docs/RUNBOOK.md` → "Service Environment and Credential Isolation".

## Resource policy

The drop-in limits the control-plane services while leaving the runner enough
room for one bounded build workload. The limits are intentionally explicit so
an accidental unbounded test or build cannot consume the host indefinitely:

| Service | MemoryHigh | MemoryMax | TasksMax |
| --- | ---: | ---: | ---: |
| API | 512M | 1G | 256 |
| CI | 2G | 4G | 512 |
| Git server | 512M | 1G | 256 |
| Runner | 8G | 12G | 1024 |

The candidate CI unit owns the scheduler HTTP API, so scheduler resource
accounting is included in the CI limits. The standalone scheduler row from
older deployments is intentionally absent.

`MemoryAccounting` and `TasksAccounting` are enabled for every unit. No CPU
quota is imposed in this first policy revision because runner children execute
the actual build workload; CPU limits must be tuned from measured Fedora
behavior rather than guessed.

## Cross-process Git-to-CI contract

Git-server and CI are separate user services. Configure the Git-server unit
with `DATABASE_URL` (the Git-server variable), `GIT_ROOT`,
`GITFORGE_CI_TRIGGER_URL`, and `GITFORGE_CI_TRIGGER_TOKEN`. Configure the CI
unit with `GITFORGE_DATABASE_URL`, `GITFORGE_TRIGGER_TOKEN`, and the scheduler
tokens. In a standard deployment the trigger URL is:

```text
http://127.0.0.1:42781/pipelines/trigger
```

The trigger token must be identical to the CI `GITFORGE_TRIGGER_TOKEN`.
`DATABASE_URL` and `GITFORGE_DATABASE_URL` are not interchangeable in the
current binaries; setting only the latter leaves Git-server repository lookup
disabled and causes Git discovery to return 503.

The Git-server bridge parses successful receive-pack updates, persists one
`ci.trigger.pending` event per ref in the shared `events` table, and retries
delivery until CI acknowledges it. A push can still succeed while CI is down
because the Git ref has already been accepted, but the pending event survives
service restart and is delivered by the Git-server outbox worker when CI
recovers. Monitor pending events and delivery age in production.

For machine-readable monitoring, run
`scripts/gitforge-outbox-status "$GITFORGE_DATABASE_URL"`. It returns JSON
with pending/delivering counts and the oldest active event age; a delivering
event causes `status` to become `attention` until it is completed or returned
to pending.

Before assembling a release bundle, run
`scripts/gitforge-release-preflight <release-source-root>`. It fails closed
unless `api`, `ci`, `git-server`, and `runner` are all present and executable.
The candidate CI binary owns the scheduler HTTP API, so the legacy standalone
`gitforge-scheduler-service` must not be mixed into the bundle.

The legacy `make run-all` and `make stop` targets intentionally refuse
unmanaged background startup and broad process termination. Service lifecycle
belongs to the user service manager — today the named `gitforge-*.service`
units — so resource limits, restart behavior, and status remain observable
and scoped to named GitForge units. A managed user unit still inherits the
manager's imported environment unless its per-unit `UnsetEnvironment=` policy
removes irrelevant provider variables; the live units do not yet have that
drop-in. Ad-hoc copies started from an operator shell inherit the same ambient
environment and are outside unit-level protection. Always use named service
units and install the verified per-unit scrub before restarting them.

## Credential isolation

A user service manager passes its whole login environment to the services it
starts, so every exported provider key in the operator shell would reach
GitForge processes and everything they spawn. `gitforge-env-isolation.conf`
is a drop-in that scrubs the provider/host credential set with
`UnsetEnvironment=`; systemd applies it as the final step when compiling the
executed environment, so it wins over `EnvironmentFile=` files and imported
login variables. The list is mirrored from the canonical list in
`systemd/gitforge@.service`; `scripts/verify-unit-env-policy` (run by
`make unit-policy`, part of `make lint`) fails on drift between the two
files and on any scrub that would remove a variable a service actually
reads. This removes provider keys/endpoints, not all cross-service GitForge
secrets: JWT, database, scheduler, and CI-trigger credentials still need a
later per-unit least-privilege policy. The exact per-service environment
contract and remaining gap are documented in `docs/RUNBOOK.md`.

Two credential paths are intentionally outside every service unit:

- **Interactive CLI** — `gitforge` code review reads `ANTHROPIC_API_KEY` /
  `OPENAI_API_KEY` from the invoking shell. It is not a service; the scrub
  never applies to it.
- **Job payloads** — credentials a CI job needs travel in the job
  specification through the scheduler/runner API and are injected as
  explicit container env pairs; the runner's own process environment is
  never forwarded into job containers.

## Atomic release pointer

`gitforge-release-bundle` creates immutable releases with executables under
`bin/`. The service examples therefore resolve binaries through
`%h/work/gitforge-current/bin/`. Validate a release first, then preview the
pointer change:

```text
scripts/gitforge-release-promote \
  <release-directory> <fedora-work-root>/gitforge-current
```

The command is dry-run by default and refuses any pointer whose basename is
not `gitforge-current` or whose existing target is not a symlink. After a
separate canary decision, repeat the exact command with `--apply`; it creates
a temporary symlink next to the pointer and uses an atomic rename to switch
the pointer. The prior resolved target is printed for rollback planning. This
command does not restart services, alter systemd units, or change production
unless an operator explicitly invokes `--apply` against a production path.

## Validation and rollout

1. Copy the drop-ins (`gitforge-resource-limits.conf` per service, plus
   `gitforge-env-isolation.conf`) into each matching `*.service.d/` directory
   in a disposable user manager or candidate account.
2. Run `systemd-analyze --user verify` against every unit and drop-in.
3. Start a candidate GitForge bundle with isolated ports/database/workspace.
4. Run the serialized DB/API/scheduler/runner gates and the push smoke test.
5. Check `scripts/gitforge-status --json` and confirm reported policy values.
6. Promote one release atomically, health-check, and retain the prior release
   for rollback.

Do not restart production units with this policy until the CI queue is drained,
the runner's container-child accounting and rollback procedure have been
verified, and the full remote GitForge lane passes.
