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
| Scheduler | 512M | 1G | 256 |
| Runner | 8G | 12G | 1024 |

`MemoryAccounting` and `TasksAccounting` are enabled for every unit. No CPU
quota is imposed in this first policy revision because runner children execute
the actual build workload; CPU limits must be tuned from measured Fedora
behavior rather than guessed.

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
