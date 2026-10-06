# GitForge CI integration

The GitForge workflow is opt-in and runs only on the self-hosted
`gitforge-github` runner. GitHub-hosted runners cannot reach the private Fedora
LAN. When that runner is on the Fedora GitForge host, use the loopback scheduler
URL; if it is moved elsewhere, require a deliberately secured reachable route.

Before enabling it, verify that the expected self-hosted runner is online and
that the configured repository UUID belongs to this GitForge repository.
Configure these repository or organization values:

| Name | Kind | Requirement |
| --- | --- | --- |
| `GITFORGE_ENABLED` | variable | Exactly `true`; keep false until every row is verified |
| `GITFORGE_SCHEDULER_URL` | variable | Scheduler base URL; `http://127.0.0.1:42781` when the runner is co-located with Fedora GitForge |
| `GITFORGE_REPO_ID` | variable | UUID of this GitForge repository, not GitHub's repository ID |
| `GITFORGE_POLL_TIMEOUT_SECONDS` | variable | Positive integer timeout |
| `GITFORGE_POLL_INTERVAL_SECONDS` | variable | Positive integer interval |
| `GITFORGE_CI_TRIGGER_TOKEN` | secret | Value accepted by `POST /pipelines/trigger` |
| `GITFORGE_SCHEDULER_OPERATOR_TOKEN` | secret | Value accepted by `GET /pipelines/runs/{id}` |

The trigger and status endpoints deliberately use different credentials. The
trigger handler accepts the CI trigger token; scheduler run inspection uses
the operator credential. The workflow polls the scheduler's durable
`/pipelines/runs/{id}` route, not the API gateway's user-authenticated
`/api/pipeline-runs/{id}` route. Its contract test prevents those routes or
credential roles from drifting. The workflow refuses to bypass GitForge when
enqueue or polling fails. Until all configuration is independently verified
and `GITFORGE_ENABLED=true` is explicitly set, it skips the bridge.

For push and tag events, the workflow forwards GitHub's short ref name and commit
range. For same-repository pull requests, it forwards the mirrored head branch
(`head-ref`), base SHA, and head SHA; GitHub's synthetic
`<number>/merge` ref is not a GitForge mirror ref. Fork pull requests fail
closed because the canonical GitForge mirror does not contain the fork's head
branch. Before enabling this event path, verify that the mirror synchronizes
same-repository feature branches as well as protected branches.

The trigger service can accept an event without returning a durable run ID
when correlation expires under load. The workflow treats that response as a
failure and does not retry the trigger (which could create duplicate builds)
or report a passing GitForge gate. A durable event-to-run lookup or idempotent
trigger contract is required before treating this bridge as resilient under
queue contention.

The Fedora-native GitForge path remains the source-of-truth CI path for local
Git pushes. The GitHub workflow is only an integration bridge and must not be
treated as proof of Fedora service health.

## Supplementary dependency review

The GitHub Dependency Review action is also opt-in through the repository or
organization variable `DEPENDENCY_REVIEW_ENABLED=true`. The current repository
does not expose the dependency-graph capability required by that action, so
the gate remains skipped. `cargo audit` is the mandatory dependency security
gate until GitHub support is enabled and independently verified.
