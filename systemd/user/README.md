# GitForge Fedora user-systemd policy

These files describe the deployment contract for the Fedora host. They are
candidate configuration and are not installed automatically. The live Fedora
deployment uses user units under `~/.config/systemd/user/`; apply changes only
through a release/rollback procedure after validating the complete unit set.

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
belongs to user-systemd so resource limits, restart behavior, and status remain
observable and scoped to named GitForge units.

## Validation and rollout

1. Copy the drop-in into each matching `*.service.d/` directory in a disposable
   user manager or candidate account.
2. Run `systemd-analyze --user verify` against every unit and drop-in.
3. Start a candidate GitForge bundle with isolated ports/database/workspace.
4. Run the serialized DB/API/scheduler/runner gates and the push smoke test.
5. Check `scripts/gitforge-status --json` and confirm reported policy values.
6. Promote one release atomically, health-check, and retain the prior release
   for rollback.

Do not install this policy directly into the current production units until the
runner's container-child accounting and the rollback procedure have been
verified.
