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
| `GITFORGE_API_URL` | variable | Reachable GitForge API base URL |
| `GITFORGE_SCHEDULER_URL` | variable | Reachable scheduler base URL |
| `GITFORGE_REPO_ID` | variable | UUID of the mirrored GitForge repository |
| `GITFORGE_POLL_TIMEOUT_SECONDS` | variable | Positive integer timeout |
| `GITFORGE_POLL_INTERVAL_SECONDS` | variable | Positive integer interval |
| `GITFORGE_TRIGGER_TOKEN` | secret | Dedicated scheduler trigger credential |
| `GITFORGE_READ_API_TOKEN` | secret | API user token with repository and pipeline read access |

The enqueue and read credentials are intentionally separate: the scheduler
accepts its dedicated trigger token, while receipt and pipeline-run routes use
the API's authenticated-user middleware. Reusing the trigger secret for API
polling can enqueue successfully and then fail with HTTP 401. For migration,
`GITFORGE_TRIGGER_TOKEN` falls back to the existing
`GITFORGE_CI_TRIGGER_TOKEN`, then the previous `GITFORGE_API_TOKEN` secret.
The read token must be provisioned separately.

The workflow validates all values and refuses to bypass GitForge if enqueue or
polling fails. Until `GITFORGE_ENABLED=true` is intentionally configured, the
workflow is skipped rather than issuing requests to an invalid local endpoint.

The Fedora-native GitForge path remains the source-of-truth CI path for local
Git pushes. The GitHub workflow is only an integration bridge and must not be
treated as proof of Fedora service health.

## Supplementary dependency review

The GitHub Dependency Review action is also opt-in through the repository or
organization variable `DEPENDENCY_REVIEW_ENABLED=true`. The current repository
does not expose the dependency-graph capability required by that action, so
the gate remains skipped. `cargo audit` is the mandatory dependency security
gate until GitHub support is enabled and independently verified.
