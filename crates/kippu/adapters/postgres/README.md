# kippu-store-postgres

PostgreSQL adapter for [`kippu-store`](../../store), for deployments with many instances and
heavy contention. PostgreSQL 13 or newer.

- **Concurrent writers.** Transactions run at READ COMMITTED and never serialize in the
  application. Stock and quotas move by conditional `UPDATE`s, whose `WHERE` PostgreSQL
  re-checks after waiting for a row lock, so `held + sold` never exceeds capacity; a
  `CHECK` constraint backs that up.
- **Claiming work** uses `FOR UPDATE SKIP LOCKED`: many workers drain the purchase queue in
  parallel without blocking on each other's batches.
- **Outbox order.** Appends take a transaction-scoped advisory lock, so sequence numbers are
  assigned in commit order and consumers that remember the last sequence never skip an event
  that committed late.
- **Schema** in [`migrations/`](migrations): native UUIDs, `TIMESTAMPTZ` (microseconds, like
  Kippu's timestamps), `JSONB` for small structured values, indexes on every foreign key that
  is used for lookups.
- **Conformance:** `EKIBEN_TEST_POSTGRES_URL=postgres://… cargo test -p kippu-store-postgres`
  runs the shared suite, each case in a schema of its own; without the variable the cases are
  skipped.

```text
database.url = "postgres://kippu:secret@db.internal/kippu?max_connections=32"
```

`max_connections` (default 16) sets the pool size per instance; every other parameter is
passed to libpq as usual (`sslmode`, `application_name`, …).

Waiting-room positions come from a per-sale counter row incremented by a single atomic
`INSERT … ON CONFLICT DO UPDATE`; positions must be dense per sale for admission batches to
mean "the next N people", which a shared sequence would not give.
