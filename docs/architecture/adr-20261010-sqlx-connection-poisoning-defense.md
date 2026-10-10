# ADR: Two-Layer Defense Against sqlx Connection Poisoning

**Status:** Accepted — 2026-10-10
**Evidence:** commits 0b16dd76, 0dfc0f15; incident runs `99c463db`
(swallowed writes), `4d07b0a9` (nested-BEGIN); ledger items F21/F23.

## Context

sqlx 0.9 tracks transaction depth in a per-connection counter, but the
SQLite handle can end up inside a transaction the counter does not know
about: when `BEGIN IMMEDIATE` fails under write contention, the failing
connection returns to the pool with its handle still in a transaction
while the counter reads zero. Every later borrow of that connection is
poisoned in one of two ways:

1. **Explicit transactions fail** — the next `begin_with` hits
   `cannot start a transaction within a transaction`; retries are
   futile because the pool hands connections out LIFO and retries
   re-acquire the same poisoned connection.
2. **Autocommit writes are swallowed** — a single-statement write routed
   to the poisoned connection joins the orphaned transaction and
   disappears when that transaction is rolled back. Run `99c463db`
   logged `persisted planned job rows planned=4` yet zero rows existed.

`persist_with_retry` (F21/F23 discipline) cannot see either failure
mode: the nested-BEGIN error does surface as a retryable
`ErrorKind::Database`, but retries re-acquire the same poisoned
connection; the swallowed write reports success.

## Decision

Defense lives in `gitforge-db`, in two layers:

1. **`begin_immediate(pool, context)`** — every explicit-transaction
   path opens through this helper. On a failed begin it acquires a
   connection and issues a bare `ROLLBACK` recovery pass (up to 3), then
   retries the begin. Bounded: three passes then the error propagates.
2. **Pool-level `before_acquire` hook** (`heal_poisoned_connection`) —
   installed on every pool `Pool::new` builds. At the idle→borrowed
   boundary: a `ROLLBACK` that succeeds proves an orphaned transaction
   existed and heals the connection (warn logged); `no transaction is
   active` proves the connection healthy; any other failure retires the
   connection (`Ok(false)` closes it). This layer covers the swallowed-
   write class, which no explicit-transaction helper can reach.

## Consequences

- Swallowed-write data loss of the `99c463db` class is closed for every
  caller, including plain queries that never open transactions.
- The hook adds one round-trip of `ROLLBACK` per connection borrow.
  Measured negligible on the validation instance (SQLite local file);
  if it ever matters, a cached in-connection flag can short-circuit.
- Any future non-SQLite backend must re-evaluate the probe semantics.
- Regression tests pin both classes: a desynced-connection recovery
  test (max-1 pool, raw BEGIN poison) and a cross-connection
  swallowed-write test asserting the write survives close/reopen only
  with the hook installed.
