# ADR: CI Trigger Correlation and Status

**Status:** Accepted as a transitional contract — 2026-10-07
**Scope:** CI trigger API and its caller-facing run correlation.

## Context

The trigger API may accept work before the event consumer has created a
pipeline run. Returning `202 queued` without a way to resolve the event later
leaves callers unable to correlate the accepted request with its run or a
terminal creation failure. Increasing the synchronous wait is not a delivery
guarantee and creates request-latency coupling to runner/control-plane load.

The current event bus is in-memory. Persisting status rows in the existing
event table can recover correlation state after a restart, but it does not
persist or replay the trigger payload. Therefore a `202` response is not a
durable-delivery receipt.

## Decision

1. Every accepted trigger response includes an immutable `event_id`.
2. If a run is created within the bounded correlation window, the response
   returns `accepted` and its `pipeline_run_id`; otherwise it returns `queued`
   and the caller polls `GET /pipelines/trigger/{event_id}`.
3. The status endpoint returns only the correlation fields needed by callers:
   `queued`, `accepted` with the run ID, or terminal `failed`. It does not
   echo repository or commit metadata.
4. Trigger status is held in a bounded in-process cache and appended to the
   existing event journal when the database is configured. The journal is a
   best-effort status ledger, not an outbox.
5. Trigger authentication selects one credential by precedence:
   `GITFORGE_CI_TRIGGER_TOKEN`, `GITFORGE_TRIGGER_TOKEN`, then legacy
   scheduler operator/shared tokens. Once a newer credential is configured,
   older credentials are no longer accepted by the trigger control plane, so
   rotating the active secret revokes it. CI uses the CI trigger credential
   for trigger POST and trigger-status polling; scheduler-operator credentials
   remain available for run-status polling.
6. Request workspace handoffs are keyed by event ID and consumed exactly
   once, so concurrent triggers for the same repository cannot overwrite
   each other's workspace selection. If event publication fails, the trigger
   is recorded failed and its workspace handoff and run waiter are released.
7. Consumer outcomes are recorded only for event IDs registered by the trigger
   endpoint and still pending. Ordinary non-deletion push events share the
   same event type but must not create trigger journal rows.

## Consequences

- Callers can resolve a queued trigger without keeping the original HTTP
  request open indefinitely.
- Run creation failure is observable as a terminal trigger state rather than
  an unbounded poll for a run that will never exist.
- Correlation state is restart-recoverable only when journal writes succeed;
  the trigger itself may still be lost if the service crashes after returning
  `202` and before the in-memory event is consumed.
- The current contract must not be marketed as reliable/durable trigger
  delivery or treated as production acceptance for crash-safe CI enqueue.
- The durable outbox and replay mechanism is a separate required follow-up;
  it must provide atomic trigger persistence, idempotent consumption, and
  recoverable dispatch before the durability gap is closed. See
  [the outbox implementation handoff](../handoffs/GITFORGE_TRIGGER_OUTBOX_DURABILITY_2026-10-07.md).

## Rejected alternatives

- **Return `202` as if it proved durable enqueue:** rejected because the
  in-memory bus can lose the payload on process failure.
- **Keep the HTTP request open until every pipeline run exists:** rejected as
  the primary contract because request duration is coupled to service load
  and does not address process-crash delivery loss.
- **Use a repository-keyed workspace cache:** rejected because concurrent
  requests for one repository can replace each other's handoff.

## Validation requirements

- Contract tests pin response JSON, event ID, status transitions, terminal
  failure, and workspace isolation/consumption.
- The current PR head must pass the Fedora GitForge Rust gates before merge.
- The durable outbox follow-up requires failure-injection evidence for crash
  between acceptance and consumption, duplicate delivery, restart recovery,
  and exactly-once run creation (or documented at-least-once delivery with
  idempotency).
