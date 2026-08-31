# Resource-safe service lifecycle

GitForge service management must be scoped to one checkout. A restart or
validation run must never terminate processes belonging to another repository,
another worktree, or the host operating system.

## Required behavior

- Identify services by the kernel-resolved `/proc/<pid>/exe` matching an exact
  executable path under `GITFORGE_ROOT/target/release`; service arguments are
  therefore supported without substring matching.
- Send `TERM` first and allow `GITFORGE_TERM_WAIT_SECONDS` (default `10`) for
  graceful shutdown.
- Send `KILL` only to the same exact executable if it remains after the grace
  period.
- Treat PID files as disposable metadata, never as permission to kill a PID.
- Keep generated logs, profiles, coverage, and build artifacts under the
  checkout or a GitForge-managed artifact root; never use shared `/tmp`.
- Run only one repository validation job per constrained runner unless an
  explicit capacity policy says otherwise.
- Host workspaces are retained by default. Automatic cleanup is opt-in and
  requires `GITFORGE_AUTO_CLEANUP_WORKSPACES=true` plus
  `GITFORGE_WORKSPACE_ROOT`; only a single immediate child of that root, shared
  by all terminal jobs in a pipeline, may be removed after parent finalization.
  Caller-owned or ambiguous paths are refused and logged.

## Operator command

```bash
GITFORGE_ROOT=/path/to/checkout \
GITFORGE_TERM_WAIT_SECONDS=20 \
  ./scripts/stop-services.sh
```

The command is safe to run when no service is running. It is intentionally not
implemented with `pkill -f` or command-line substring matching because those can
kill unrelated test runners and agent processes.

## Validation contract

Changes to lifecycle scripts require:

1. `bash -n scripts/stop-services.sh`.
2. An empty-root smoke test proving no unrelated process is selected.
3. A controlled TERM/KILL test using disposable service processes, with the
   process tree and listeners checked before and after.
4. Remote Fedora validation through the GitForge runner or an isolated checkout;
   never by stopping the production checkout without a maintenance window.
