# GitForge CI integration

The GitForge workflow is opt-in. It must not default to `localhost`: GitHub
hosted runners cannot reach the private Fedora host where the GitForge control
plane runs.

Enable the integration only when one of these network arrangements is in
place:

1. A GitHub self-hosted runner is installed on the Fedora host and the
   workflow is changed to target that runner label; or
2. GitForge is exposed through a deliberately secured, reachable endpoint
   with TLS, authentication, and firewall policy reviewed.

Configure these repository or organization values before enabling it:

| Name | Kind | Requirement |
| --- | --- | --- |
| `GITFORGE_ENABLED` | variable | Exactly `true` |
| `GITFORGE_SCHEDULER_URL` | variable | Reachable scheduler base URL |
| `GITFORGE_REPO_ID` | variable | UUID of the mirrored GitForge repository |
| `GITFORGE_POLL_TIMEOUT_SECONDS` | variable | Positive integer timeout |
| `GITFORGE_POLL_INTERVAL_SECONDS` | variable | Positive integer interval |
| `GITFORGE_TRIGGER_TOKEN` | secret | Dedicated scheduler trigger credential |

`GITFORGE_TRIGGER_TOKEN` authenticates enqueue and the scheduler's
read-only `GET /pipeline-trigger-requests/{request_id}` receipt endpoint. The
receipt contains only request state, correlated run ID, and pipeline state;
it does not require an API-user JWT or expose general pipeline listings. The
endpoint must be deployed on the scheduler before enabling this workflow.
For migration, `GITFORGE_TRIGGER_TOKEN` falls back to the existing
`GITFORGE_CI_TRIGGER_TOKEN` secret. The workflow intentionally does not fall
back to the general `GITFORGE_API_TOKEN`; configure a dedicated trigger token
instead of reusing broader API credentials.

The workflow validates its inputs and refuses to bypass GitForge if enqueue or
polling fails. It polls the durable receipt until both trigger materialization
and the pipeline reach a terminal state. Until `GITFORGE_ENABLED=true` is
intentionally configured, it is skipped rather than issuing requests to an
invalid endpoint.

The Fedora-native GitForge path remains the source-of-truth CI path for local
Git pushes. The GitHub workflow is only an integration bridge and must not be
treated as proof of Fedora service health.

## Supplementary dependency review

The GitHub Dependency Review action is also opt-in through the repository or
organization variable `DEPENDENCY_REVIEW_ENABLED=true`. The current repository
does not expose the dependency-graph capability required by that action, so
the gate remains skipped. `cargo audit` is the mandatory dependency security
gate until GitHub support is enabled and independently verified.
