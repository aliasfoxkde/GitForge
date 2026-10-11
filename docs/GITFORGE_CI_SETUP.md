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
| `GITFORGE_GIT_URL` | variable | Git HTTP URL of the SAME repository (`…/:42782/<owner>/<repo>.git`); must be the repo behind `GITFORGE_REPO_ID` |
| `GITFORGE_POLL_TIMEOUT_SECONDS` | variable | Positive integer timeout |
| `GITFORGE_POLL_INTERVAL_SECONDS` | variable | Positive integer interval |
| `GITFORGE_CI_TRIGGER_TOKEN` | secret | Bearer token for `POST /pipelines/trigger` and trigger correlation retries only |
| `GITFORGE_SCHEDULER_OPERATOR_TOKEN` | secret | Bearer token for run-status reads (`GET /pipelines/runs/{id}`) only |
| `GITFORGE_GIT_USERNAME` | secret | Git HTTP username for commit-registration pushes to `GITFORGE_GIT_URL` |
| `GITFORGE_GIT_TOKEN` | secret | Git HTTP password/token for those pushes; distinct from both scheduler credentials |

### Least privilege

The workflow holds two distinct scheduler credentials, never one shared
token:

- `GITFORGE_CI_TRIGGER_TOKEN` is presented ONLY to `POST /pipelines/trigger`
  — the enqueue step and the correlation re-POSTs of the poll job (re-POSTs
  must carry the same credential as the original enqueue so the scheduler's
  dedup echo contract applies).
- `GITFORGE_SCHEDULER_OPERATOR_TOKEN` is presented ONLY to `GET
  /pipelines/runs/{id}` — the status-poll loop. The scheduler rejects the
  trigger credential on this route (401). Neither secret is used for the
  gateway route `/api/pipeline-runs/{id}`, which requires a user JWT the
  workflow deliberately never holds.

The workflow validates both secrets are present and distinct without
printing their values; identical values fail the run rather than silently
re-creating a shared super-token.

The workflow validates all values and refuses to bypass GitForge if enqueue or
polling fails. Until `GITFORGE_ENABLED=true` is intentionally configured, the
workflow is skipped rather than issuing requests to an invalid local endpoint.

## Commit registration (why GITFORGE_GIT_* exists)

GitForge runs read the commit from the repository's registered git storage:
the run workspace is cloned from it, and the pipeline definition is loaded
from the pushed revision (`git cat-file <sha>:<config>`) against it. A GitHub
pull request's merge commit (`refs/pull/N/merge`, and `github.sha` with it)
exists only on GitHub — enqueueing a trigger before the commit is in GitForge
storage mints a run that fails at checkout ("unable to read tree"), observed
live 2026-10-11 (trigger `dc3382f1`, run `89d2a80e`, GitHub run 38106742159).

The `Register commit in GitForge storage` step closes that gap before the
enqueue POST is allowed, using ordinary receive-pack pushes — the same
mechanism every developer push uses:

1. Fetch `github.ref` (for pull_request events that is `refs/pull/N/merge`;
   full history, never shallow — a shallow push would install a shallow
   boundary in the instance's storage) from the GitHub mirror and require the
   fetched head to equal `github.sha`.
2. Push it to `GITFORGE_GIT_URL` under the per-SHA scratch ref
   `refs/ci/github-bridge/<sha>`: content-addressed, never force-pushed,
   never moves a branch or tag, safe under concurrent PRs. The push fires a
   native trigger for the ref update, which the trigger endpoint dedups onto
   the same `(repo_id, new_hash)` tuple as the workflow's own POST — at most
   one active run per commit.
3. Verify `git ls-remote` serves that scratch ref at exactly the event's
   SHA. Only then is `POST /pipelines/trigger` sent.

Consequences: a missing or unreachable repository fails the step before any
run exists (the scheduler-side repository-storage preflight remains the
backstop and still fails such triggers terminally with no run row), and the
scratch refs accumulate one ref per built SHA — a GC candidate on the
instance, like any other CI by-product. The push credential is a dedicated
user account with write access to that one repository, kept distinct from
both scheduler credentials.

The Fedora-native GitForge path remains the source-of-truth CI path for local
Git pushes. The GitHub workflow is only an integration bridge and must not be
treated as proof of Fedora service health.

## Supplementary dependency review

The GitHub Dependency Review action is also opt-in through the repository or
organization variable `DEPENDENCY_REVIEW_ENABLED=true`. The current repository
does not expose the dependency-graph capability required by that action, so
the gate remains skipped. `cargo audit` is the mandatory dependency security
gate until GitHub support is enabled and independently verified.
