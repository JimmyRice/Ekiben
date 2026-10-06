# kippu-nats

NATS JetStream for Kippu, implementing two optional ports of [`kippu-store`](../../store):

- **Purchase inbox** (`PurchaseInbox`). Submitting a purchase publishes it to the
  `<PREFIX>_PURCHASES` work-queue stream (`Nats-Msg-Id` = the request id) and answers `202` as
  soon as JetStream has it; workers on every instance pull from one durable consumer, insert
  the requests into the database and acknowledge them. Bursts land in JetStream instead of the
  database. The `Location` of the `202` carries a signed receipt, so polling answers `queued`
  even before a worker has persisted the request.
- **Event bus** (`EventBus`). The `event-relay` task publishes every outbox event to
  `<prefix>.events.<topic>` in the `<PREFIX>_EVENTS` stream, with `Nats-Msg-Id`
  `outbox-<sequence>` and a `Kippu-Sequence` header. Relays resume from the last message's
  header, so the position needs no table; several relays at once are harmless, because
  JetStream drops repeated ids and each relay publishes in ascending order, which keeps the
  stream in outbox order.

```toml
[queue]
url = "nats://nats.internal:4222"
purchase_inbox = true
relay_events = true
```

Streams and the consumer are created on start if missing. Repeats are dropped within the
duplicate window (ten minutes by default, `NatsOptions::duplicate_window`).

Tests: `EKIBEN_TEST_NATS_URL=nats://localhost:4222 cargo test -p kippu-nats`; without the
variable they are skipped. The core HTTP tests exercise the same ports with in-memory fakes.
