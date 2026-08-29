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
| `GITFORGE_API_TOKEN` | secret | Token accepted by the scheduler/API |

The workflow validates all values and refuses to bypass GitForge if enqueue or
polling fails. Until `GITFORGE_ENABLED=true` is intentionally configured, the
workflow is skipped rather than issuing requests to an invalid local endpoint.

The Fedora-native GitForge path remains the source-of-truth CI path for local
Git pushes. The GitHub workflow is only an integration bridge and must not be
treated as proof of Fedora service health.
