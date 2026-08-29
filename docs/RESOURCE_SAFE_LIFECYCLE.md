# Resource-safe service lifecycle

GitForge service management must be scoped to one checkout. A restart or
validation run must never terminate processes belonging to another repository,
another worktree, or the host operating system.

## Required behavior

- Identify services by an exact executable path under `GITFORGE_ROOT/target/release`.
- Send `TERM` first and allow `GITFORGE_TERM_WAIT_SECONDS` (default `10`) for
  graceful shutdown.
- Send `KILL` only to the same exact executable if it remains after the grace
  period.
- Treat PID files as disposable metadata, never as permission to kill a PID.
- Keep generated logs, profiles, coverage, and build artifacts under the
  checkout or a GitForge-managed artifact root; never use shared `/tmp`.
- Run only one repository validation job per constrained runner unless an
  explicit capacity policy says otherwise.

## Operator command

```bash
GITFORGE_ROOT=/path/to/checkout \
GITFORGE_TERM_WAIT_SECONDS=20 \
  ./scripts/stop-services.sh
```

The command is safe to run when no service is running. It is intentionally not
implemented with `pkill -f` because command-line substring matching can kill
unrelated test runners and agent processes.

## Validation contract

Changes to lifecycle scripts require:

1. `bash -n scripts/stop-services.sh`.
2. An empty-root smoke test proving no unrelated process is selected.
3. A controlled TERM/KILL test using disposable service processes, with the
   process tree and listeners checked before and after.
4. Remote Fedora validation through the GitForge runner or an isolated checkout;
   never by stopping the production checkout without a maintenance window.
