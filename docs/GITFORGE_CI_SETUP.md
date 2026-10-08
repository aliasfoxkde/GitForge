# GitForge CI trigger-correlation contract

How the `gitforge-ci` workflow correlates a build it enqueued with the CI
orchestrator that runs it. Deployment enablement (variables, secrets, runner
networking) lives in `.github/GITFORGE_CI_SETUP.md`; this document is the
endpoint and secret contract those settings buy.

The `ci_trigger_requests` table and indexes belong to
`gitforge-db::Pool::migrate`; the CI service owns request lifecycle behavior,
not a parallel schema-creation path. Database-backed service startup runs
the shared migration before binding the listener.

## Enqueue: `POST /pipelines/trigger`

Credential: `GITFORGE_CI_TRIGGER_TOKEN`, sent as
`Authorization: Bearer <token>` or the git-server compatibility header
`x-gitforge-trigger-token: <token>`.

The request is recorded durably before anything is planned. A deduplicated
repeat answers `202` immediately; a new request holds the connection for at
most `CI_TRIGGER_CORRELATION_WINDOW` (15 seconds in current builds) while the
orchestrator plans the run. Either way the answer is `202` and always carries
a **stable `trigger_id`** (a UUID), including on a deduplicated request.
The response includes `status`, `trigger_id`, `deduplicated`, `event_id`,
`pipeline_run_id`, `repo_id`, and `new_hash`; `pipeline_run_id` is absent
until a run has been correlated.

| `status` | Meaning |
| --- | --- |
| `accepted` | The run was planned and `pipeline_run_id` was correlated inside the trigger window. |
| `queued` | The correlation window (see above) elapsed before the waiter fired. By design the in-process consumer still plans the run afterwards, so the caller polls the `trigger_id` for its id — but see the coverage note below. |
| `deduplicated` | A request for the same repository, ref, and commit is still open; no new run was planned, and `pipeline_run_id` is absent. |

Coverage note: the integration tests exercise the `accepted` and
`deduplicated` answers end to end. The `queued` answer itself is still not
exercised — no test holds a live trigger past the correlation window on the
wire. The window's passage is simulated by backdating database rows:
integration-side for a row whose run is already linked (dedup survives the
window) and for an unlinked `pending` row whose expiry is now observed
through the real status endpoint with the service's own durable expiry
verdict. That the orchestrator still plans a run whose caller already saw
`queued` is the implementation's stated intent, unverified.

Error responses: `400` for a malformed body (`invalid_repo_id`,
`invalid_new_hash`, `invalid_workspace`), `401` for a wrong or missing
trigger credential, `503` `trigger_consumer_unavailable` when the
in-process trigger-event consumer is not live (see "Consumer liveness and
startup ordering" below), and `503` when the trigger credential is not
configured
(`trigger_auth_not_configured`), the submit and read credentials are
configured as the same value (`trigger_auth_misconfigured`), the event bus
rejects the publish (`event_publish_failed`), or the durable
trigger-request store cannot be written
(`trigger_request_store_unavailable`). A store failure is
fail-closed: no run is planned, because a run the caller can never correlate
is the defect this contract removes.

The `400` codes above are the handler's own validation, returned as the JSON
envelope. A request the web framework rejects before the handler runs never
produces them: invalid JSON syntax and a missing or non-JSON `content-type`
are Axum extractor rejections (plain-text `400` and `415` respectively), and
a body that parses but fails deserialization — missing or wrongly-typed
fields — is a plain-text `422`. Clients must not expect the JSON envelope on
those.

### Consumer liveness and startup ordering

An accepted trigger is published to an in-process bus; a broadcast bus
delivers nothing to a subscriber that does not exist yet. The submit endpoint
is therefore fail-closed about consumer liveness: while the trigger-event
consumer has no live bus subscription — before boot completes, and across
every supervised restart window — `POST /pipelines/trigger` answers `503`
`trigger_consumer_unavailable` instead of accepting a trigger whose run
could never appear. The `503` is retryable: the supervisor restarts the
consumer with a backoff that doubles from 250 ms to a 30 s cap, and the
endpoint reopens once the replacement's subscription is live.

Startup ordering enforces the same contract structurally: the consumer task
is spawned **before** the HTTP listener binds, and `main` waits until the
consumer reports a live subscription before binding any socket. A consumer
that cannot subscribe within 30 seconds is treated as a broken bus, and the
service exits with an error rather than come up unable to deliver triggers.
The boot-cutoff instant for the restart sweep is captured after this wait
and before the listener binds, so every `claimed` row the boot sweep can
touch provably predates this process.

`trigger_consumer_unavailable` is asserted at the handler level in a unit
test; no integration test drives the endpoint during a restart gap on the
wire.

A CI service started without `GITFORGE_DATABASE_URL` keeps its historical
in-memory mode. What the integration tests verify is narrow: the service
starts and serves `/health`, and the trigger-request status endpoint answers
`503` `trigger_store_unavailable` — there is nothing durable to poll. That a
trigger POST in this mode still plans a run, answering `202` with no
`trigger_id` (the id exists only when a durable request is recorded), is the
implementation's intent — the claim step treats a missing store as license to
proceed — but no test drives a trigger through the endpoint without a
database, so treat that part as intended/unverified.

## Status: `GET /pipelines/trigger-requests/{trigger_id}`

Credential: `GITFORGE_SCHEDULER_OPERATOR_TOKEN`, sent as
`Authorization: Bearer <token>`. The shared scheduler token
(`GITFORGE_SCHEDULER_TOKEN`) is accepted only when no operator token is
configured, mirroring `GET /pipelines/runs/{run_id}`; the operator-first
resolution order is unit-tested at the credential resolver, while acceptance
of the shared token itself is not exercised end to end.

```json
{
  "trigger_id": "0d3fc1a2-…",
  "status": "processing",
  "repo_id": "…",
  "ref_name": "refs/heads/main",
  "new_hash": "…",
  "pipeline_run_id": "c5829e10-…",
  "error": null,
  "created_at": "2026-10-08T00:00:00+00:00",
  "updated_at": "2026-10-08T00:00:05+00:00"
}
```

| Status | Meaning |
| --- | --- |
| `pending` | Recorded; the run has not been planned yet. |
| `claimed` | Transitional: a consumer received the event and owns the request, but has not linked a run yet. Short-lived in practice, but it can surface on a poll. Recovery for a lost claim happens at the start of each consumer attempt: the in-process sweep closes a claim only when provably no run row exists behind it (no reserved run id, or a reserved id with no `pipeline_runs` row); a run-backed claim is never failed — it resolves through the run itself via read-time grading. The boot-time restart sweep additionally closes unlinked claims created at or before process boot, whose in-memory event died with the previous process. The tests poll through `claimed` to `processing` without asserting on it. |
| `processing` | The run exists and has not finished; `pipeline_run_id` is set. |
| `completed` | The linked run finished successfully. |
| `failed` | The linked run finished unsuccessfully, or the trigger could not be planned; `error` carries the cause. |

Error responses: `400` `invalid_trigger_id` (not a UUID), `401`
`trigger_request_auth_required` (wrong or missing credential), `404`
`trigger_request_not_found`, `503` `trigger_store_unavailable` (the CI
service runs without `GITFORGE_DATABASE_URL`) or
`trigger_request_auth_not_configured` (no operator credential), and `500`
`trigger_request_lookup_failed` if the store read itself errors. All but the
`500` are exercised by the integration tests.

Responses carry correlation and lifecycle evidence only — never a credential.
Statuses are graded against the durable run row when one is linked: an open
run reads `processing`, and a terminal run verdict grades the request
`completed` or `failed` at read time. The grading rules themselves are
unit-tested (including the mapping from terminal run verdicts and
stored-verdict finality), but the case this grading exists for — a run that
reached a verdict while the trigger-status write was lost to contention or a
restart, healed at read time — is not covered by any test: no test reads a
non-terminal request whose linked run row is already terminal. Treat the
lost-write heal as intended/unverified.

### Why polling instead of re-submitting

Deduplication is deliberately narrow: a repeat trigger collapses into the
open request only while that request can still resolve — `processing`
(durable run, watchdog finalizes it) or `pending` inside the correlation
window (the consumer plans the run or fails the request within it). A
terminal request is closed, so a repeat trigger for the same push is a **new
build**, not a status query. A workflow that wants status must therefore poll
the `trigger_id` it was given; re-submitting to "check" starts duplicate
builds. The same narrowness keeps recovery live: a `pending` request older
than the correlation window lost its in-memory event (service restart), so it
never blocks a re-submission.

## Secret contract

| Capability | Credential | Header |
| --- | --- | --- |
| Submit a build (`POST /pipelines/trigger`) | `GITFORGE_CI_TRIGGER_TOKEN` | `Authorization` or `x-gitforge-trigger-token` |
| Read request lifecycle (`GET /pipelines/trigger-requests/{id}`) | `GITFORGE_SCHEDULER_OPERATOR_TOKEN` | `Authorization` |
| Read run status (`GET /pipelines/runs/{id}`) | `GITFORGE_SCHEDULER_OPERATOR_TOKEN` | `Authorization` |

The two credentials are enforced per endpoint, not by convention: the trigger
credential cannot read the lifecycle (its dedicated header is not consulted
on read routes), and the operator credential cannot submit work. Configure
them as two different secrets — a single value used for both voids the
separation by definition, and the service enforces that: while the effective
submit and read credentials resolve to the same value, both endpoints fail
closed with `503` `trigger_auth_misconfigured`. Configuring only one role
keeps that role's documented behavior, with the other endpoint answering its
usual not-configured `503`. Neither token is valid at the API gateway — the
deployment guide is explicit that neither credential may be used there.

Secrets live only in the service environment and the platform secret store,
and they are never committed. Responses never echo them: the integration
tests assert that no response body contains a configured credential value,
and the status payload has no credential-shaped field at all (unit-tested).
"Never logged" is a code property, not a tested one — the test harness spawns
the service with stdout and stderr discarded (`Stdio::null()`), so no service
log is captured anywhere in the suite and log hygiene can only be verified by
reading the code.

## Test evidence

`services/ci/tests/ci_trigger_flow.rs` drives the real `ci` binary against a
temporary SQLite store and covers, end to end: trigger auth on both header
forms (`test_trigger_requires_token_and_runs_committed_pipeline`); the
`accepted` and `deduplicated` answers plus credential separation on both
endpoints (`test_trigger_request_lifecycle_and_credential_separation`); a
linked request graded terminal by its run's durable verdict
(`test_linked_trigger_request_reads_terminal_after_the_run_verdict`); dedup
across a backdated correlation window
(`test_repeat_trigger_stays_deduplicated_past_the_correlation_window`);
the status endpoint's missing-store and auth `503`s
(`test_trigger_request_status_reports_a_missing_store`,
`test_trigger_request_status_requires_the_operator_credential`); the
identical-credential fail-closed
(`test_identical_submit_and_read_credentials_fail_closed`); the boot restart
sweeps for abandoned `claimed` and stale `pending` rows with a terminal row
left untouched
(`test_restart_sweep_closes_claims_lost_to_the_previous_process`); and a
`pending` request expiring through the real status endpoint with the
service's own durable correlation-window verdict and no planned run
(`test_queued_trigger_request_expires_after_the_correlation_window_while_running`).

Unit tests in `services/ci/src/main.rs` cover the credential resolvers, the
grading rules (`grade_trigger_status_follows_the_linked_run`,
`grade_trigger_status_keeps_a_terminal_request_and_unknown_run_statuses`),
the trigger-request state machine, the handler-level
`trigger_consumer_unavailable` fail-closed, and the consumer supervision
lifecycle: acceptance opens only after a live subscription and reopens after
both panic and error-return restarts
(`consumer_attempt_opens_acceptance_after_subscribing_and_recovers_dead_claims`,
`supervised_consumer_keeps_a_serving_worker_open_until_shutdown`), and the
per-attempt recovery sweep fails only claims without a run row while a
run-backed claim resolves through the run's verdict
(`claim_recovery_fails_only_claims_without_a_run_and_run_verdicts_close_the_rest`).
Recovery-store failure and subscription-health cleanup are covered by
`recovery_sweep_failure_propagates_instead_of_reporting_zero` and
`subscription_guard_marks_consumer_health_down_on_drop_and_panic`. The shared
migration test also asserts that the trigger-request table and indexes are
created and that rerunning the migration is idempotent (`test_migrations`).

Nothing covers the `queued` response, read-time healing of a lost terminal
trigger-status write, or a trigger POST without a database — and the harness
discards the service's stdout and stderr, so the suite observes no logs.

## Validation status

Source formatting passed with `cargo fmt -p ci -- --check`; `git diff --check`
also passed. No build, `cargo test`, Clippy, or GitForge pipeline has run
against these changes. The test names above describe source coverage, not
passing results; treat behavior as unverified until focused tests, workspace
gates, and the self-hosted GitForge pipeline have run.
