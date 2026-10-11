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
| `GITFORGE_CI_TRIGGER_TOKEN` | secret | Bearer token for `POST /pipelines/trigger` and trigger correlation retries only |
| `GITFORGE_SCHEDULER_OPERATOR_TOKEN` | secret | Bearer token for run-status reads (`GET /pipelines/runs/{id}`) only |

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

### Correlation contract

A queued enqueue answers `202` with a durable `trigger_id` and a null
`pipeline_run_id`; the run is created by the asynchronous consumer. The poll
job correlates by re-POSTing the tuple with `trigger_id` in the body: the
scheduler then reads that durable request's state and mints nothing.

- `accepted` + `pipeline_run_id` — the request settled with its run; poll it.
- `failed` + `error` — the request failed terminally; the recorded cause is
  actionable and the gate stays red.
- `queued` + null run id — still in flight (or the active-row dedup echo);
  keep polling until the correlation deadline.

Addressing the request by id is the only channel that outlives settlement:
the tuple dedup matches only `pending`/`processing` rows, so an unaddressed
re-POST after the consumer completes or fails a request mints a fresh trigger
(and a duplicate event) instead of surfacing the original outcome — the
2026-10-10 shape where run 38106742159 passed while the poll job reported
`trigger_lost`. A scheduler predating this contract ignores the field and
echoes a different trigger id; the workflow fails closed on that signal.

The workflow validates both secrets are present and distinct without
printing their values; identical values fail the run rather than silently
re-creating a shared super-token.

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
