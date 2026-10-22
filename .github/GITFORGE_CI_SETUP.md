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
| `GITFORGE_GIT_URL` | variable | Smart-HTTP Git URL for that repository |
| `GITFORGE_POLL_TIMEOUT_SECONDS` | variable | Positive integer timeout |
| `GITFORGE_POLL_INTERVAL_SECONDS` | variable | Positive integer interval |
| `GITFORGE_API_TOKEN` | secret | Token accepted by the scheduler/status API |
| `GITFORGE_GIT_USERNAME` | secret | GitForge Git transport username |
| `GITFORGE_GIT_TOKEN` | secret | Token accepted by Git smart-HTTP push |

The workflow checks out the candidate commit and prefers the helper already
present on the repository default branch. On the bootstrap PR that first adds
the helper, it uses the same-repository candidate copy; fork PRs are skipped
and receive no GitForge credentials. The helper publishes the commit to a
unique `github-ci/` candidate ref in GitForge, verifies the remote object ID,
then enqueues it. It accepts a valid HTTP 202 trigger receipt and polls the
authenticated trigger status endpoint until a durable run ID is assigned. It
refuses to bypass GitForge if publication, enqueue, or polling fails.
Candidate refs are retained for audit; a bounded retention/cleanup policy is a
follow-up and refs must not be deleted while a queued/running pipeline may
still need them. Until `GITFORGE_ENABLED=true` is intentionally configured,
the workflow is skipped rather than issuing requests to an invalid local
endpoint.

The Fedora-native GitForge path remains the source-of-truth CI path for local
Git pushes. The GitHub workflow is only an integration bridge and must not be
treated as proof of Fedora service health.

## Supplementary dependency review

The GitHub Dependency Review action is also opt-in through the repository or
organization variable `DEPENDENCY_REVIEW_ENABLED=true`. The current repository
does not expose the dependency-graph capability required by that action, so
the gate remains skipped. `cargo audit` is the mandatory dependency security
gate until GitHub support is enabled and independently verified.
