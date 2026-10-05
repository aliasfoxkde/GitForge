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
| `GITFORGE_SCHEDULER_URL` | variable | Reachable CI (scheduler) service base URL; both the trigger and the trigger-status endpoint are answered there |
| `GITFORGE_REPO_ID` | variable | UUID of the mirrored GitForge repository |
| `GITFORGE_POLL_TIMEOUT_SECONDS` | variable | Positive integer timeout |
| `GITFORGE_POLL_INTERVAL_SECONDS` | variable | Positive integer interval |
| `GITFORGE_TRIGGER_TOKEN` | secret | Token the CI service accepts for `POST /pipelines/trigger` |
| `GITFORGE_STATUS_TOKEN` | secret | Dedicated token for `GET /pipelines/trigger/status/{event_id}`; must differ from `GITFORGE_TRIGGER_TOKEN` |

`GITFORGE_API_URL` and `GITFORGE_API_TOKEN` are no longer used by the
workflow: run status is answered by the CI service itself, keyed by the
`event_id` returned at trigger time, so no API gateway JWT is held. The two
secrets must be distinct high-entropy values; the workflow refuses to start
when either is missing, when both hold the same value, or when the trigger
response does not carry a well-formed (canonical 8-4-4-4-12 hexadecimal)
event id. All UUID inputs — `GITFORGE_REPO_ID`, the trigger response's
`event_id` and `pipeline_run_id`, and the poll step's event id — are
validated against that canonical shape.

Note on the trigger credential: during the migration to dedicated tokens the
CI service still accepts its scheduler operator/shared token names at the
trigger endpoint, so the operator token must be treated as trigger-capable.
Configure a dedicated `GITFORGE_TRIGGER_TOKEN` rather than reusing the
operator secret. The status endpoint has no such fallback and accepts only
`GITFORGE_STATUS_TOKEN`. See `docs/RUNBOOK.md` for the service-side
credential table.

Status authorization is a shared-token trust boundary, not an event-scoped
one. The workflow only ever polls the `event_id` its own trigger call
returned, but the CI service does not cryptographically bind
`GITFORGE_STATUS_TOKEN` to that event or to the repository: any caller
holding the status token can read the lifecycle state of any stored trigger
event by UUID. UUID secrecy is not a substitute for token auth. Treat the
status token with the same care as the trigger token, and treat the status
endpoint's reachability as part of the exposure surface. Per-event and
per-repository (multi-tenant) authorization on the status endpoint is
future work, to be added only if a deployment actually needs it.

While polling, non-terminal and transient answers keep the job alive: only
`succeeded` is green, an unrecognized body fails the job, and a retryable
503 (for example a transient status-read error) is retried until the
`GITFORGE_POLL_TIMEOUT_SECONDS` deadline.

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
