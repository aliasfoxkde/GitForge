# GitForge CI integration

The GitForge workflow is opt-in and self-hosted. Its jobs run on the Fedora
runner with the label set `[self-hosted, linux, x64, gitforge-github]`, and the
scheduler URL is expected to be the loopback CI/scheduler HTTP API on that same
host (default `http://127.0.0.1:42781`) so trigger and status traffic never
leaves the box. The workflow must not default to `localhost` on GitHub-hosted
runners: they cannot reach the private Fedora host where the GitForge control
plane runs, so the integration stays skipped until `GITFORGE_ENABLED=true` is
set deliberately.

Enable the integration only when one of these network arrangements is in
place:

1. The GitHub self-hosted runner is installed on the Fedora host (current
   arrangement; the workflow already targets the `gitforge-github` label); or
2. GitForge is exposed through a deliberately secured, reachable endpoint
   with TLS, authentication, and firewall policy reviewed, and the runner
   labels are adjusted to match.

Configure these repository or organization values before enabling it:

| Name | Kind | Requirement |
| --- | --- | --- |
| `GITFORGE_ENABLED` | variable | Exactly `true` |
| `GITFORGE_SCHEDULER_URL` | variable | Loopback scheduler base URL on the self-hosted runner host (e.g. `http://127.0.0.1:42781`) |
| `GITFORGE_REPO_ID` | variable | UUID of the mirrored GitForge repository |
| `GITFORGE_POLL_TIMEOUT_SECONDS` | variable | Positive integer; run-status polling budget |
| `GITFORGE_POLL_INTERVAL_SECONDS` | variable | Positive integer; sleep between polls |
| `GITFORGE_CORRELATION_TIMEOUT_SECONDS` | variable | Positive integer; budget for resolving a queued event to a pipeline run (optional, default `600`) |
| `GITFORGE_CI_TRIGGER_TOKEN` | secret | Trigger credential; accepted by `POST /pipelines/trigger` and `GET /pipelines/events/{event_id}` |
| `GITFORGE_SCHEDULER_OPERATOR_TOKEN` | secret | Operator credential; accepted by `GET /pipelines/runs/{run_id}` |

The two credentials are deliberately separate: the trigger token may start
runs and resolve trigger events to runs, while only the operator token reads
run and job status.

## Trigger and correlation contract

`POST /pipelines/trigger` answers:

- HTTP 202 with `pipeline_run_id` set (`status: "accepted"`): the durable run
  row exists and the workflow polls `/pipelines/runs/{run_id}` immediately.
- HTTP 202 with `pipeline_run_id: null` (`status: "queued"`): the event was
  accepted but the consumer has not correlated a run within the server's
  correlation window. The event is **not** lost. The workflow then polls
  `GET /pipelines/events/{event_id}` (trigger credential) until it answers
  `correlated` with a `pipeline_run_id`, `failed` with a reason, or its
  correlation budget expires. The server keeps correlation records for 24
  hours.
- Anything else (including HTTP 500 `event_processing_failed`) is treated as
  an enqueue failure.

The workflow never re-triggers the same event because correlation was
delayed, and it reports success only after the correlated run reaches a
successful terminal state through `/pipelines/runs/{run_id}`. Queued alone is
never treated as success. Transient transport errors and HTTP 408, 429, or
5xx responses are retried until the applicable budget expires. HTTP 404
(unknown run or event), credential rejections, other HTTP errors, and unknown
run statuses fail immediately.


The trigger API currently has no caller-supplied idempotency key. If the POST
is accepted but its response is lost, this workflow fails without retrying the
POST; a manual workflow rerun can create a duplicate run. Do not describe this
as exactly-once delivery. Durable idempotency and correlation across service
restarts remain follow-up requirements for production-grade recovery.

The workflow validates all values and refuses to bypass GitForge if enqueue,
correlation, or polling fails. Until `GITFORGE_ENABLED=true` is intentionally
configured, the workflow is skipped rather than issuing requests to an invalid
local endpoint.

If the CI service restarts after accepting an event but before the run is
correlated, the in-memory event and its correlation record are lost; the
workflow's correlation lookup then answers 404 and the run fails closed
instead of silently passing.

The Fedora-native GitForge path remains the source-of-truth CI path for local
Git pushes. The GitHub workflow is only an integration bridge and must not be
treated as proof of Fedora service health.

## Supplementary dependency review

The GitHub Dependency Review action is also opt-in through the repository or
organization variable `DEPENDENCY_REVIEW_ENABLED=true`. The current repository
does not expose the dependency-graph capability required by that action, so
the gate remains skipped. `cargo audit` is the mandatory dependency security
gate until GitHub support is enabled and independently verified.
