# GitForge CI integration

The GitForge workflow is opt-in. It must not default to `localhost`: GitHub
hosted runners cannot reach the private Fedora host where the GitForge control
plane runs.

Enable the integration only when one of these network arrangements is in
place:

1. A GitHub self-hosted runner is installed on the Fedora host with the
   labels already required by the workflow (`self-hosted`, `linux`, `x64`,
   `gitforge-github`); or
2. GitForge is exposed through a deliberately secured, reachable endpoint
   with TLS, authentication, and firewall policy reviewed.

Configure these repository or organization values before enabling it:

For the current Fedora self-hosted runner, the GitForge scheduler/CI control
plane is reachable at `http://127.0.0.1:42781`. This loopback value is valid
only because the jobs run on the Fedora host; do not use it with GitHub-hosted
runners. The API gateway at port `42780` is not used by this workflow.

| Name | Kind | Requirement |
| --- | --- | --- |
| `GITFORGE_ENABLED` | variable | Exactly `true` |
| `GITFORGE_SCHEDULER_URL` | variable | Reachable scheduler base URL |
| `GITFORGE_REPO_ID` | variable | UUID of the mirrored GitForge repository |
| `GITFORGE_POLL_TIMEOUT_SECONDS` | variable | Positive integer timeout |
| `GITFORGE_POLL_INTERVAL_SECONDS` | variable | Positive integer interval |
| `GITFORGE_CI_TRIGGER_TOKEN` | secret | Credential accepted by the CI trigger endpoint for `POST /pipelines/trigger` |
| `GITFORGE_SCHEDULER_OPERATOR_TOKEN` | secret | Scheduler operator credential for `GET /pipelines/runs/{run_id}` status polling |

The credentials are intentionally separate: enqueue uses the CI trigger token;
status polling uses the scheduler operator token. Do not substitute one for
the other or use either token with the API gateway.

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
