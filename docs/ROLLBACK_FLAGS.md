# Rollout Flags

Deployment-level switches for surfaces that ship dark. Each flag has a safe
default (off) and can be enabled per deployment without code changes. Flags
are read ONCE at service startup (the edge) and passed into the code that
needs them as data — no decision path reads the environment itself.

## `GITFORGE_ENABLE_FAULT_INJECTION` — fault injection (M10)

**Default: off.**

Enables the pipeline `fault:` job key and the runner's failure-simulation
path. Used by CI pipeline testing (M10) to exercise the scheduler → runner →
grading path for each failure shape without burning real compute or waiting
for a real timeout.

### When off (default)

- `PipelineDefinition::parse` (the path every trigger and API pipeline-save
  uses) **rejects** any pipeline containing a `fault:` job key with an error
  naming the key and this flag. Nothing degrades silently.
- The runner never simulates: even a fault-bearing definition that reached a
  runner through another route carries `fault: None` end to end, because the
  parse gate is the only producer of `Some(_)`.

### When on (`GITFORGE_ENABLE_FAULT_INJECTION=1`)

Accepted values: `1`, `true`, `yes` (case-sensitive, surrounding whitespace
trimmed). Anything else — including `0`, `false`, `on` — keeps the surface
off.

A job may then carry:

```yaml
jobs:
  - name: graded-timeout
    image: dsc-ci-rust:11
    fault: timeout      # graded TimedOut, exit -1
    steps: [...]
  - name: graded-failure
    image: dsc-ci-rust:11
    fault: nonzero-exit # graded Failed, exit 1
    steps: [...]
  - name: graded-oom
    image: dsc-ci-rust:11
    fault: oom          # graded Failed, exit 137 (128 + SIGKILL)
    steps: [...]
```

The runner simulates the outcome and executes **no steps**; the job result
error names the injection explicitly (`fault injection: simulated '…'`), so
a green-eyes run-log review cannot mistake it for an organic failure.

### Rollback

Unset the flag on the API and CI services and restart. Existing pipelines
with `fault:` keys stop parsing (loudly) at the next trigger; the flag only
ever gates new parses, so a rollback cannot strand in-flight runs.

### Set it

```bash
# systemd drop-in for the api and ci units
Environment=GITFORGE_ENABLE_FAULT_INJECTION=1
```

The runner binary does not need the flag: it simulates whatever the
scheduler's assignment carries, and the scheduler only carries faults that
survived the parse gate.
